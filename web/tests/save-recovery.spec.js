const { test, expect } = require("@playwright/test");
const { randomUUID } = require("node:crypto");
const { writeFile } = require("node:fs/promises");
const operator = "freight-test-operator-credential-at-least-32-chars";
const sourceOperator = "freight-test-quote-source-credential-at-least-32-chars";

test.beforeEach(async ({ page }) => {
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned synthetic fixtures",
  );
  await page.clock.install();
});
async function signIn(page, token = operator) {
  await page.goto("/");
  await page.getByLabel("Workspace credential").fill(token);
  await page
    .getByRole("button", { name: "Open workspace", exact: true })
    .click();
  await expect(
    page.getByRole("heading", { name: "Overview", exact: true }),
  ).toBeVisible();
}
async function workspace(page) {
  const response = await page.request.get("/api/freight/workspace");
  expect(response.ok()).toBe(true);
  return response.json();
}
async function customerForm(page, name) {
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Customers", exact: true })
    .click();
  await page.getByRole("button", { name: "Add customer", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Customer name", { exact: true }).fill(name);
  await dialog
    .getByLabel("Contact email or phone")
    .fill("owned-save@example.test");
  return dialog;
}
async function record(testInfo, name, value) {
  await writeFile(testInfo.outputPath(name), JSON.stringify(value, null, 2));
}
const recovery = (page) =>
  page.getByRole("region", { name: "Save recovery", exact: true });
const uniqueName = () => `Owned recovery ${randomUUID().slice(0, 8)}`;

for (const committed of [true, false]) {
  test(`an interrupted customer save ${committed ? "after" : "before"} commit replays its original body and key without duplicate records`, async ({
    page,
  }, testInfo) => {
    await signIn(page);
    const name = uniqueName(),
      dialog = await customerForm(page, name),
      attempts = [];
    await page.route("**/api/freight/customers", async (route) => {
      const request = route.request();
      const attempt = {
        key: request.headers()["idempotency-key"],
        body: request.postDataJSON(),
      };
      attempts.push(attempt);
      if (attempts.length === 1 && !committed) return route.abort("failed");
      const response = await route.fetch();
      attempt.status = response.status();
      attempt.result = await response.json();
      if (attempts.length === 1) return route.abort("failed");
      await route.fulfill({ response });
    });
    await dialog
      .getByRole("button", { name: "Save record", exact: true })
      .click();
    await expect(
      dialog
        .getByRole("alert")
        .filter({ hasText: /Failed to fetch|Save outcome not confirmed/ })
        .first(),
    ).toBeVisible();
    let current = await workspace(page);
    expect(
      current.customers.filter((customer) => customer.name === name),
    ).toHaveLength(committed ? 1 : 0);
    // A successful background refresh cannot establish the original request's outcome.
    const refreshed = page.waitForResponse(
      (r) => new URL(r.url()).pathname === "/api/freight/workspace",
    );
    await page.clock.fastForward(30000);
    await (await refreshed).finished();
    if (committed)
      await expect(
        page.getByRole("heading", { name, exact: true, includeHidden: true }),
      ).toBeVisible();
    // Exercise the preceding UI's editable retry to retain the actual duplicate
    // caused by changing a lost-response request, rather than just a missing label.
    if (await dialog.getByLabel("Customer name", { exact: true }).isEnabled()) {
      await dialog
        .getByLabel("Customer name", { exact: true })
        .fill(`${name} edited`);
      await dialog
        .getByRole("button", { name: "Save record", exact: true })
        .click();
    } else {
      await expect(recovery(page)).toContainText("Save outcome not confirmed");
      await recovery(page)
        .getByRole("button", { name: "Check original save", exact: true })
        .click();
    }
    await expect(dialog).toHaveCount(0);
    current = await workspace(page);
    const rows = current.customers.filter((customer) =>
      customer.name.startsWith(name),
    );
    await record(
      testInfo,
      `lost-customer-${committed ? "committed" : "uncommitted"}.json`,
      { name, attempts, customers: rows },
    );
    expect(rows).toHaveLength(1);
    expect(rows[0].name).toBe(name);
    expect(attempts).toHaveLength(2);
    expect(attempts[1].key).toBe(attempts[0].key);
    expect(attempts[1].body).toEqual(attempts[0].body);
    if (committed) {
      expect(attempts[1].result.data).toEqual(attempts[0].result.data);
      expect(attempts[1].result.replayed).toBe(true);
    }
    await expect(recovery(page)).toHaveCount(0);
  });
}

test("a confirmed save with failed refresh stays explicit and recovers using reads only", async ({
  page,
}, testInfo) => {
  await signIn(page);
  const name = uniqueName(),
    dialog = await customerForm(page, name),
    attempts = [];
  await page.route("**/api/freight/customers", async (route) => {
    const response = await route.fetch();
    attempts.push({
      key: route.request().headers()["idempotency-key"],
      body: route.request().postDataJSON(),
      status: response.status(),
      result: await response.json(),
    });
    await route.fulfill({ response });
  });
  await page.route("**/api/freight/workspace?history=deferred", (route) =>
    route.fulfill({
      status: 503,
      contentType: "application/json",
      body: JSON.stringify({
        error: { message: "Owned workspace refresh outage" },
      }),
    }),
  );
  await dialog
    .getByRole("button", { name: "Save record", exact: true })
    .click();
  await expect(dialog).toHaveCount(0);
  const rows = (await workspace(page)).customers.filter(
    (customer) => customer.name === name,
  );
  await record(testInfo, "confirmed-refresh-outage.json", {
    name,
    attempts,
    customers: rows,
  });
  expect(rows).toHaveLength(1);
  await expect(recovery(page)).toContainText("Save completed");
  await expect(
    page.getByRole("button", { name: "Add customer", exact: true }),
  ).toBeDisabled();
  const retry = recovery(page).getByRole("button", {
    name: "Refresh workspace",
    exact: true,
  });
  await retry.click();
  await expect(retry).toBeEnabled();
  await expect(recovery(page)).toContainText("Save completed");
  expect(attempts).toHaveLength(1);
  await page.unroute("**/api/freight/workspace?history=deferred");
  await retry.click();
  await expect(recovery(page)).toHaveCount(0);
  await expect(page.getByRole("heading", { name, exact: true })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Add customer", exact: true }),
  ).toBeEnabled();
  expect(attempts).toHaveLength(1);
});

test("an interrupted tender replays its reviewed version after refresh without a second EDI job", async ({
  page,
}, testInfo) => {
  await signIn(page);
  const origin = new URL(page.url()).origin,
    reference = `SAVE-${randomUUID().slice(0, 8)}`;
  const date = new Date(Date.now() + 86400000).toISOString().slice(0, 10);
  const createdResponse = await page.request.post("/api/freight/loads", {
    headers: { Origin: origin, "Idempotency-Key": randomUUID() },
    data: {
      reference,
      customer_id: "c1",
      mode: "FTL",
      equipment: "Dry van",
      commodity: "Owned tender save recovery",
      weight_lbs: 1000,
      miles: 173,
      buy_cents: 125001,
      sell_cents: 150001,
      origin: {
        facility: "Owned pickup",
        city: "Portland",
        state: "OR",
        date,
        address_lines: ["10 Synthetic Save Street"],
        postal_code: "97201",
        country: "US",
      },
      destination: {
        facility: "Owned delivery",
        city: "Seattle",
        state: "WA",
        date,
        address_lines: ["20 Synthetic Save Street"],
        postal_code: "98101",
        country: "US",
      },
    },
  });
  expect(createdResponse.ok(), await createdResponse.text()).toBe(true);
  const original = (await createdResponse.json()).data;
  await page.reload();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /Load board/ })
    .click();
  await page.getByLabel("Search loads").fill(reference);
  await page.getByRole("button", { name: reference, exact: true }).click();
  const dialog = page.getByRole("dialog"),
    attempts = [];
  await dialog.getByRole("button", { name: "Tender", exact: true }).click();
  await dialog.getByLabel("Carrier", { exact: true }).selectOption("ca4");
  await page.route(
    `**/api/freight/loads/${original.id}/commands`,
    async (route) => {
      const response = await route.fetch();
      attempts.push({
        key: route.request().headers()["idempotency-key"],
        body: route.request().postDataJSON(),
        status: response.status(),
        result: await response.json(),
      });
      if (attempts.length === 1) return route.abort("failed");
      await route.fulfill({ response });
    },
  );
  await dialog
    .getByRole("button", { name: "Confirm tender", exact: true })
    .click();
  await expect(
    dialog
      .getByRole("alert")
      .filter({ hasText: /Failed to fetch|Save outcome not confirmed/ })
      .first(),
  ).toBeVisible();
  const refreshed = page.waitForResponse(
    (r) => new URL(r.url()).pathname === "/api/freight/workspace",
  );
  await page.clock.fastForward(30000);
  await (await refreshed).finished();
  await expect(
    page
      .getByRole("row", { includeHidden: true })
      .filter({ hasText: reference }),
  ).toContainText("Tendered");
  await expect(recovery(page)).toContainText("Save outcome not confirmed");
  await recovery(page)
    .getByRole("button", { name: "Check original save", exact: true })
    .click();
  await expect(dialog).toHaveCount(0);
  const current = await workspace(page),
    load = current.loads.find((value) => value.id === original.id),
    jobs = current.outbox.filter(
      (job) =>
        job.load_id === original.id && job.destination === "stateset-edi",
    );
  await record(testInfo, "lost-tender.json", {
    original,
    attempts,
    load,
    jobs,
  });
  expect(attempts).toHaveLength(2);
  expect(attempts[0].key).toBe(attempts[1].key);
  expect(attempts[0].body).toEqual(attempts[1].body);
  expect(attempts[1].body.expected_version).toBe(original.version);
  expect(attempts[0].result.data).toEqual(attempts[1].result.data);
  expect(attempts[1].result.replayed).toBe(true);
  expect(load.version).toBe(original.version + 1);
  expect(load.status).toBe("tendered");
  expect(jobs).toHaveLength(1);
});

test("a validation rejection remains editable and a corrected save uses a new key", async ({
  page,
}, testInfo) => {
  await signIn(page);
  const name = uniqueName(),
    dialog = await customerForm(page, "X".repeat(201)),
    attempts = [];
  await page.route("**/api/freight/customers", async (route) => {
    const response = await route.fetch();
    attempts.push({
      key: route.request().headers()["idempotency-key"],
      status: response.status(),
    });
    await route.fulfill({ response });
  });
  await dialog
    .getByRole("button", { name: "Save record", exact: true })
    .click();
  await expect(dialog.getByRole("alert")).toContainText("1–200");
  await expect(recovery(page)).toHaveCount(0);
  await expect(
    dialog.getByLabel("Customer name", { exact: true }),
  ).toBeEnabled();
  await dialog.getByLabel("Customer name", { exact: true }).fill(name);
  await dialog
    .getByRole("button", { name: "Save record", exact: true })
    .click();
  await expect(dialog).toHaveCount(0);
  expect(attempts.map((value) => value.status)).toEqual([422, 200]);
  expect(attempts[0].key).not.toBe(attempts[1].key);
  expect(
    (await workspace(page)).customers.filter(
      (customer) => customer.name === name,
    ),
  ).toHaveLength(1);
  await record(testInfo, "validation-retry.json", { attempts, name });
});

test("sign out clears unresolved saves and fences a late original-save response", async ({
  page,
}, testInfo) => {
  await signIn(page, sourceOperator);
  const name = uniqueName(),
    dialog = await customerForm(page, name),
    attempts = [];
  let release, seen;
  const held = new Promise((resolve) => {
    release = resolve;
  });
  const started = new Promise((resolve) => {
    seen = resolve;
  });
  await page.route("**/api/freight/customers", async (route) => {
    const response = await route.fetch();
    attempts.push({
      key: route.request().headers()["idempotency-key"],
      result: await response.json(),
    });
    if (attempts.length === 1) return route.abort("failed");
    seen();
    await held;
    await route.fulfill({ response });
  });
  await dialog
    .getByRole("button", { name: "Save record", exact: true })
    .click();
  await expect(recovery(page)).toContainText("Save outcome not confirmed");
  await dialog
    .getByRole("button", { name: "Close dialog", exact: true })
    .click();
  await expect(recovery(page)).toContainText("Save outcome not confirmed");
  await recovery(page)
    .getByRole("button", { name: "Check original save", exact: true })
    .click();
  await started;
  await page.getByRole("button", { name: /Operations team.*Sign out/ }).click();
  await expect(page.getByLabel("Workspace credential")).toBeVisible();
  await expect(page.locator(".app-shell")).toHaveCount(0);
  release();
  await page.clock.runFor(100);
  await expect(recovery(page)).toHaveCount(0);
  await signIn(page, sourceOperator);
  await expect(recovery(page)).toHaveCount(0);
  expect(
    (await workspace(page)).customers.filter(
      (customer) => customer.name === name,
    ),
  ).toHaveLength(1);
  expect(attempts[0].key).toBe(attempts[1].key);
  await record(testInfo, "signed-out-save.json", { name, attempts });
});

test("an unreadable committed response and rejected check keep the original save until its receipt is recovered", async ({
  page,
}, testInfo) => {
  await signIn(page);
  const name = uniqueName(),
    dialog = await customerForm(page, name),
    attempts = [];
  await page.route("**/api/freight/customers", async (route) => {
    const attempt = {
      key: route.request().headers()["idempotency-key"],
      body: route.request().postDataJSON(),
    };
    attempts.push(attempt);
    if (attempts.length === 2) {
      attempt.status = 403;
      return route.fulfill({
        status: 403,
        contentType: "application/json",
        body: JSON.stringify({
          error: { message: "Owned original-save check denied" },
        }),
      });
    }
    const response = await route.fetch();
    attempt.status = response.status();
    attempt.result = await response.json();
    if (attempts.length === 1)
      return route.fulfill({ response, body: "{unreadable-owned-response" });
    await route.fulfill({ response });
  });
  await dialog
    .getByRole("button", { name: "Save record", exact: true })
    .click();
  await expect(recovery(page)).toContainText("Save outcome not confirmed");
  await recovery(page)
    .getByRole("button", { name: "Check original save", exact: true })
    .click();
  await expect(recovery(page)).toContainText(
    "Owned original-save check denied",
  );
  await expect(
    dialog.getByLabel("Customer name", { exact: true }),
  ).toBeDisabled();
  expect(
    (await workspace(page)).customers.filter(
      (customer) => customer.name === name,
    ),
  ).toHaveLength(1);
  await recovery(page)
    .getByRole("button", { name: "Check original save", exact: true })
    .click();
  await expect(dialog).toHaveCount(0);
  const customers = (await workspace(page)).customers.filter(
    (customer) => customer.name === name,
  );
  expect(customers).toHaveLength(1);
  expect(attempts.map((attempt) => attempt.status)).toEqual([200, 403, 200]);
  expect(new Set(attempts.map((attempt) => attempt.key)).size).toBe(1);
  expect(attempts[2].body).toEqual(attempts[0].body);
  expect(attempts[2].result.data).toEqual(attempts[0].result.data);
  expect(attempts[2].result.replayed).toBe(true);
  await record(testInfo, "malformed-save.json", { name, customers, attempts });
});
