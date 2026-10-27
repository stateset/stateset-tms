const { test, expect } = require("@playwright/test");
const { randomUUID } = require("node:crypto");
const { readFile, writeFile } = require("node:fs/promises");
const credential = "freight-test-operator-credential-at-least-32-chars";

test.beforeEach(async ({ page }) => {
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned synthetic fixtures",
  );
  await page.goto("/");
  await page.getByLabel("Workspace credential").fill(credential);
  await page
    .getByRole("button", { name: "Open workspace", exact: true })
    .click();
  await expect(
    page.getByRole("heading", { name: "Overview", exact: true }),
  ).toBeVisible();
});

async function fixture(page, customerName, count = 2) {
  const origin = new URL(page.url()).origin;
  const suffix = randomUUID().slice(0, 8);
  const prefix = `CSV-${suffix}`;
  const post = async (path, data) => {
    const response = await page.request.post(`/api/freight/${path}`, {
      headers: { Origin: origin, "Idempotency-Key": randomUUID() },
      data,
    });
    expect(response.ok(), await response.text()).toBe(true);
    return (await response.json()).data;
  };
  const customer = await post("customers", {
    name: customerName,
    contact: "owned-export@example.test",
  });
  const date = new Date(Date.now() + 86400000).toISOString().slice(0, 10);
  const created = [];
  for (let i = 0; i <= count; i++) {
    created.push(
      await post("loads", {
        reference: i === count ? `OUTSIDE-${suffix}` : `${prefix}-${i}`,
        customer_id: i === count ? "c1" : customer.id,
        mode: "FTL",
        equipment: "Dry van",
        commodity: "Owned export qualification",
        weight_lbs: 1000,
        miles: 173,
        buy_cents: i === 0 ? 150001 : 100001,
        sell_cents: i === 0 ? 125000 : 200003,
        origin: {
          facility: "Owned pickup",
          city: "Portland",
          state: "OR",
          date,
        },
        destination: {
          facility: "Owned delivery",
          city: "Seattle",
          state: "WA",
          date,
        },
      }),
    );
  }
  await page.reload();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /Load board/ })
    .click();
  await page.getByLabel("Search loads").fill(prefix);
  await expect(page.locator("tbody tr")).toHaveCount(count);
  return {
    prefix,
    customer,
    loads: created.slice(0, count),
    excluded: created[count],
  };
}

async function download(page, testInfo, filename) {
  const ready = page.waitForEvent("download");
  await page.getByRole("button", { name: "Export", exact: true }).click();
  const file = await ready;
  expect(file.suggestedFilename()).toBe("stateset-freight-loads.csv");
  const path = testInfo.outputPath(filename);
  await file.saveAs(path);
  return readFile(path, "utf8");
}

function originalLoad(load) {
  return Object.fromEntries(
    [
      "id",
      "reference",
      "customer_id",
      "buy_cents",
      "sell_cents",
      "version",
      "updated_at",
    ].map((key) => [key, load[key]]),
  );
}

test("load CSV retains filtered rows, multiline Unicode text and numeric negative margins", async ({
  page,
}, testInfo) => {
  const owned = await fixture(page, 'Résumé Logistics, "North"\r\nDock 2');
  const csv = await download(page, testInfo, "load-board.csv");
  // Financial values stay numbers even when the margin or adjustment is negative.
  expect(csv).toContain(
    '"1500.01","1250.00","-250.01","1500.01","0.00","1250.00","0.00"',
  );
  expect(csv).toContain(
    '"1000.01","2000.03","1000.02","1000.01","0.00","2000.03","0.00"',
  );
  expect(csv).not.toContain(owned.excluded.reference);
  expect(csv).toContain('"Résumé Logistics, ""North""\r\nDock 2"');
  const response = await page.request.get("/api/freight/workspace");
  expect(response.ok()).toBe(true);
  const current = await response.json();
  for (const original of owned.loads) {
    expect(
      originalLoad(current.loads.find((load) => load.id === original.id)),
    ).toEqual(originalLoad(original));
  }
  expect(
    current.customers.find((customer) => customer.id === owned.customer.id),
  ).toEqual(owned.customer);
  await writeFile(
    testInfo.outputPath("load-board-source.json"),
    JSON.stringify(owned, null, 2),
  );
});

test("load CSV protects whitespace-prefixed formula text without modifying original customer data", async ({
  page,
}, testInfo) => {
  const owned = await fixture(page, ' \t=1+1,"CSV"\r\nMünchen', 1);
  const csv = await download(page, testInfo, "formula-text.csv");
  expect(csv).toContain('"\' \t=1+1,""CSV""\r\nMünchen"');
  const response = await page.request.get("/api/freight/workspace");
  expect(response.ok()).toBe(true);
  const current = await response.json();
  expect(
    current.customers.find((customer) => customer.id === owned.customer.id),
  ).toEqual(owned.customer);
  expect(
    originalLoad(current.loads.find((load) => load.id === owned.loads[0].id)),
  ).toEqual(originalLoad(owned.loads[0]));
  await writeFile(
    testInfo.outputPath("formula-text-source.json"),
    JSON.stringify(owned, null, 2),
  );
});

test("load CSV blocks imprecise amounts and recovers after a verified workspace refresh", async ({
  page,
}, testInfo) => {
  const owned = await fixture(page, "Owned precision guard");
  await page.route(
    "**/api/freight/workspace?history=deferred",
    async (route) => {
      const response = await route.fetch();
      const body = await response.json();
      const load = body.loads.find((item) => item.id === owned.loads[0].id);
      load.commercial.total_buy_cents = Number.MAX_SAFE_INTEGER + 1;
      await route.fulfill({ response, json: body });
    },
  );
  await page.reload();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /Load board/ })
    .click();
  await page.getByLabel("Search loads").fill(owned.prefix);
  await expect(page.locator("tbody tr")).toHaveCount(2);
  let downloads = 0;
  page.on("download", () => {
    downloads++;
  });
  const exportError = page.getByRole("alert").filter({
    hasText: "Export needs exact whole-cent amounts",
  });
  await page.getByRole("button", { name: "Export", exact: true }).click();
  await expect(exportError).toBeVisible();
  expect(downloads).toBe(0);
  await expect(page.getByLabel("Search loads")).toHaveValue(owned.prefix);
  await page.unroute("**/api/freight/workspace?history=deferred");
  const refresh = page.getByRole("button", {
    name: "Refresh load board",
    exact: true,
  });
  const refreshed = page.waitForResponse(
    (response) =>
      new URL(response.url()).pathname === "/api/freight/workspace" &&
      response.request().method() === "GET",
  );
  await refresh.click();
  expect((await refreshed).ok()).toBe(true);
  await expect(refresh).toBeEnabled();
  await expect(exportError).toHaveCount(0);
  const csv = await download(page, testInfo, "precision-retry.csv");
  expect(csv).toContain('"1500.01","1250.00","-250.01"');
  expect(downloads).toBe(1);
});
