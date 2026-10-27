const { test, expect } = require("@playwright/test");
const { randomUUID } = require("node:crypto");
const { writeFile } = require("node:fs/promises");
const credential = "freight-test-operator-credential-at-least-32-chars";
const recovery = (page) =>
  page.getByRole("region", { name: "Save recovery", exact: true });
async function setup(page, commit) {
  await page.clock.install();
  await page.goto("/");
  await page.getByLabel("Workspace credential").fill(credential);
  await page
    .getByRole("button", { name: "Open workspace", exact: true })
    .click();
  await expect(
    page.getByRole("heading", { name: "Overview", exact: true }),
  ).toBeVisible();
  const customersNavigation = page
    .getByRole("navigation")
    .getByRole("button", { name: "Customers", exact: true });
  if (page.viewportSize().width < 761)
    await page
      .getByRole("button", { name: "Toggle navigation", exact: true })
      .click();
  await customersNavigation.click();
  await page.getByRole("button", { name: "Add customer", exact: true }).click();
  const dialog = page.getByRole("dialog"),
    name = `Owned resolution ${randomUUID().slice(0, 8)}`;
  await dialog.getByLabel("Customer name", { exact: true }).fill(name);
  await dialog
    .getByLabel("Contact email or phone")
    .fill("owned-resolution@example.test");
  let original;
  await page.route("**/api/freight/customers", async (route) => {
    original = {
      key: route.request().headers()["idempotency-key"],
      body: route.request().postDataJSON(),
    };
    if (commit) {
      const response = await route.fetch();
      original.result = await response.json();
      expect(response.ok()).toBe(true);
    }
    await route.abort("failed");
  });
  await dialog
    .getByRole("button", { name: "Save record", exact: true })
    .click();
  await expect(recovery(page)).toContainText("Save outcome not confirmed");
  await recovery(page)
    .getByText("Stop waiting for this save", { exact: true })
    .click();
  await recovery(page)
    .getByLabel("Resolution note")
    .fill("Resolve the original uncertain request before another decision");
  return { name, original, dialog };
}
async function decide(page) {
  const ready = page.waitForResponse(
    (response) =>
      new URL(response.url()).pathname === "/api/freight/save-recovery/resolve",
  );
  await recovery(page)
    .getByRole("button", { name: "Resolve original save", exact: true })
    .click();
  const response = await ready;
  return {
    status: response.status(),
    result: await response.json().catch(() => null),
  };
}
async function late(page, original) {
  const response = await page.request.post("/api/freight/customers", {
    headers: {
      Origin: new URL(page.url()).origin,
      "Idempotency-Key": original.key,
    },
    data: original.body,
  });
  return { status: response.status(), result: await response.json() };
}
async function customers(page, name) {
  const response = await page.request.get("/api/freight/workspace");
  expect(response.ok()).toBe(true);
  return (await response.json()).customers.filter(
    (customer) => customer.name === name,
  );
}
async function record(testInfo, name, value) {
  await writeFile(testInfo.outputPath(name), JSON.stringify(value, null, 2));
}

test.beforeEach(() => {
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned synthetic API and original requests",
  );
});

test("resolving an unapplied save durably rejects its delayed original before any customer is created", async ({
  page,
}, testInfo) => {
  const owned = await setup(page, false);
  const resolution = await decide(page);
  const delayed = await late(page, owned.original);
  const rows = await customers(page, owned.name);
  await record(testInfo, "stopped-original.json", {
    name: owned.name,
    original: owned.original,
    resolution,
    delayed,
    customers: rows,
  });
  expect(resolution.status).toBe(200);
  expect(resolution.result.data.outcome).toBe("stopped");
  expect(delayed.status).toBe(409);
  expect(delayed.result.error.code).toBe("request_stopped");
  expect(rows).toEqual([]);
  await expect(owned.dialog).toHaveCount(0);
  await expect(recovery(page)).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Add customer", exact: true }),
  ).toBeEnabled();
});

test("resolving an already committed save retains its original result and never reverses the customer", async ({
  page,
}, testInfo) => {
  const owned = await setup(page, true);
  const resolution = await decide(page);
  const rows = await customers(page, owned.name);
  await record(testInfo, "completed-original.json", {
    name: owned.name,
    original: owned.original,
    resolution,
    customers: rows,
  });
  expect(resolution.status).toBe(200);
  expect(resolution.result.data.outcome).toBe("completed");
  expect(resolution.result.data.result).toEqual(owned.original.result.data);
  expect(rows).toEqual([owned.original.result.data]);
  await expect(owned.dialog).toHaveCount(0);
  await expect(recovery(page)).toHaveCount(0);
});

test("a lost resolution response retains its exact stop across retry and failed workspace refresh", async ({
  page,
}, testInfo) => {
  const owned = await setup(page, false),
    decisions = [];
  await page.route("**/api/freight/save-recovery/resolve", async (route) => {
    const response = await route.fetch();
    decisions.push({
      body: route.request().postDataJSON(),
      result: await response.json(),
    });
    if (decisions.length === 1) return route.abort("failed");
    await route.fulfill({ response });
  });
  await recovery(page)
    .getByRole("button", { name: "Resolve original save", exact: true })
    .click();
  await expect(recovery(page)).toContainText("Failed to fetch");
  await recovery(page)
    .getByLabel("Resolution note")
    .fill("A later retry must not replace the original recorded note");
  await page.route("**/api/freight/workspace?history=deferred", (route) =>
    route.fulfill({
      status: 503,
      contentType: "application/json",
      body: JSON.stringify({
        error: { message: "Owned refresh outage after resolution" },
      }),
    }),
  );
  await decide(page);
  await expect(owned.dialog).toHaveCount(0);
  await expect(recovery(page)).toContainText("Original save stopped");
  await expect(recovery(page)).not.toContainText("Save completed");
  expect(decisions).toHaveLength(2);
  expect(decisions[0].result.data).toEqual(decisions[1].result.data);
  expect(decisions[1].result.replayed).toBe(true);
  const delayed = await late(page, owned.original);
  expect(delayed.status).toBe(409);
  expect(delayed.result.error.code).toBe("request_stopped");
  expect(await customers(page, owned.name)).toEqual([]);
  await page.unroute("**/api/freight/workspace?history=deferred");
  await recovery(page)
    .getByRole("button", { name: "Refresh workspace", exact: true })
    .click();
  await expect(recovery(page)).toHaveCount(0);
  await record(testInfo, "repeated-resolution.json", {
    name: owned.name,
    original: owned.original,
    decisions,
    delayed,
    customers: await customers(page, owned.name),
  });
});

test("checking the original save recognizes a durable stop after its resolution response is lost", async ({
  page,
}, testInfo) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const owned = await setup(page, false),
    decisions = [];
  await page.route("**/api/freight/save-recovery/resolve", async (route) => {
    const response = await route.fetch();
    decisions.push(await response.json());
    await route.abort("failed");
  });
  await recovery(page)
    .getByRole("button", { name: "Resolve original save", exact: true })
    .click();
  await expect(recovery(page)).toContainText("Failed to fetch");
  await page.unroute("**/api/freight/customers");
  await page.route("**/api/freight/workspace?history=deferred", (route) =>
    route.fulfill({
      status: 503,
      contentType: "application/json",
      body: JSON.stringify({
        error: { message: "Owned refresh outage after stopped receipt" },
      }),
    }),
  );
  const receipt = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" &&
      new URL(response.url()).pathname === "/api/freight/customers",
  );
  await recovery(page)
    .getByRole("button", { name: "Check original save", exact: true })
    .click();
  const response = await receipt,
    stopped = await response.json();
  expect(response.status()).toBe(409);
  expect(stopped.error.code).toBe("request_stopped");
  await expect(owned.dialog).toHaveCount(0);
  await expect(recovery(page)).toContainText("Original save stopped");
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  expect(await customers(page, owned.name)).toEqual([]);
  expect(decisions).toHaveLength(1);
  await page.unroute("**/api/freight/workspace?history=deferred");
  await recovery(page)
    .getByRole("button", { name: "Refresh workspace", exact: true })
    .click();
  await expect(recovery(page)).toHaveCount(0);
  await record(testInfo, "checked-stopped-original.json", {
    name: owned.name,
    original: owned.original,
    decisions,
    stopped,
    customers: await customers(page, owned.name),
  });
});
