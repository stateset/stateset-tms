const { test, expect } = require("@playwright/test");
const { randomUUID } = require("node:crypto");
const credential = "freight-test-operator-credential-at-least-32-chars";

test("standalone catalog searches and pages without reading workspace history, then opens the authoritative shipment", async ({
  page,
}) => {
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned fixture",
  );
  test.setTimeout(90000);
  const reads = [];
  page.on("request", (request) => {
    if (request.url().includes("/api/freight/")) reads.push(request.url());
  });
  await page.goto("/catalog");
  await page.getByLabel("Workspace credential").fill(credential);
  await page.getByRole("button", { name: "Open catalog", exact: true }).click();
  await expect(
    page.getByRole("region", { name: "Shipment results" }),
  ).toBeVisible();
  const origin = new URL(page.url()).origin,
    prefix = `CATALOG-${Date.now()}`,
    date = new Date(Date.now() + 86400000).toISOString().slice(0, 10);
  for (let i = 0; i < 27; i++) {
    const response = await page.request.post("/api/freight/loads", {
      headers: { Origin: origin, "Idempotency-Key": randomUUID() },
      data: {
        reference: `${prefix}-${String(i).padStart(2, "0")}`,
        customer_id: "c1",
        mode: "FTL",
        equipment: "Dry van",
        commodity: "Owned catalog fixture",
        weight_lbs: 1000,
        miles: 173,
        buy_cents: 125000,
        sell_cents: 150000,
        origin: {
          facility: "Catalog pickup",
          city: "Portland",
          state: "OR",
          date,
          address_lines: ["15 Dock Street"],
          postal_code: "97201",
          country: "US",
        },
        destination: {
          facility: "Catalog delivery",
          city: "Seattle",
          state: "WA",
          date,
          address_lines: ["25 Dock Street"],
          postal_code: "98101",
          country: "US",
        },
      },
    });
    expect(response.ok(), await response.text()).toBe(true);
  }
  await page.getByLabel("Find shipments").fill(prefix.toLowerCase());
  await page.getByLabel("Shipment status").selectOption("planned");
  await page
    .getByRole("button", { name: "Search shipments", exact: true })
    .click();
  await expect(page.getByRole("status")).toContainText("Page 1 · 25 shipments");
  const first = await page.locator("tbody a").allTextContents();
  await page.getByRole("button", { name: "Next page", exact: true }).click();
  await expect(page.getByRole("status")).toContainText("Page 2 · 2 shipments");
  const second = await page.locator("tbody a").allTextContents();
  expect(new Set([...first, ...second]).size).toBe(27);
  await expect(
    page.getByRole("button", { name: "Next page", exact: true }),
  ).toBeDisabled();
  await page
    .getByRole("button", { name: "Previous page", exact: true })
    .click();
  await expect(page.getByRole("status")).toContainText("Page 1 · 25 shipments");
  expect(await page.locator("tbody a").allTextContents()).toEqual(first);
  expect(
    reads.some((url) => new URL(url).pathname === "/api/freight/workspace"),
  ).toBe(false);
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(
    page.getByRole("button", { name: "Search shipments", exact: true }),
  ).toBeVisible();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  const reference = first[0];
  await page.getByRole("link", { name: reference, exact: true }).click();
  await expect(page.getByRole("dialog")).toContainText(reference);
});

test("catalog discards a delayed page after sign out and blocks reopening when cookie deletion fails", async ({
  page,
}) => {
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned fixture",
  );
  await page.goto("/catalog");
  await page.getByLabel("Workspace credential").fill(credential);
  await page.getByRole("button", { name: "Open catalog", exact: true }).click();
  await expect(page.locator("tbody tr").first()).toBeVisible();
  let release;
  const held = new Promise((resolve) => {
    release = resolve;
  });
  let seen;
  const requested = new Promise((resolve) => {
    seen = resolve;
  });
  await page.route("**/api/freight/loads?**", async (route) => {
    const response = await route.fetch();
    seen();
    await held;
    await route.fulfill({ response }).catch(() => {});
  });
  await page.getByRole("button", { name: "Refresh", exact: true }).click();
  await requested;
  await page.route("**/api/session", (route) =>
    route.fulfill({
      status: 503,
      contentType: "application/json",
      body: JSON.stringify({ error: { message: "Owned deletion failure" } }),
    }),
  );
  await page.getByRole("button", { name: "Sign out", exact: true }).click();
  release();
  await expect(page.locator("main").getByRole("alert")).toContainText(
    "Sign out could not be confirmed",
  );
  await expect(
    page.getByRole("button", { name: "Open catalog", exact: true }),
  ).toBeDisabled();
  await expect(page.locator("tbody tr")).toHaveCount(0);
  await page.unroute("**/api/session");
  await page
    .getByRole("button", { name: "Retry sign out", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "Open catalog", exact: true }),
  ).toBeEnabled();
  await expect(page.locator("tbody tr")).toHaveCount(0);
});
