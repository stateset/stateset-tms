const { staffOrigin } = require("./service-origins.cjs")();
const { test, expect } = require("@playwright/test");
const { randomUUID } = require("node:crypto");
const operator = "freight-test-operator-credential-at-least-32-chars";
const other = "freight-test-quote-source-credential-at-least-32-chars";
test.use({ timezoneId: "America/Los_Angeles", locale: "en-US" });
async function signIn(page, token = operator) {
  const r = await page.request.post("/api/session", {
    headers: { Origin: staffOrigin },
    data: { token },
  });
  expect(r.ok(), await r.text()).toBe(true);
}
async function raw(page) {
  const r = await page.request.get("/api/freight/workspace");
  expect(r.ok()).toBe(true);
  return r.json();
}
async function post(page, path, data) {
  const r = await page.request.post(`/api/freight/${path}`, {
    headers: {
      Origin: staffOrigin,
      "Idempotency-Key": randomUUID(),
    },
    data,
  });
  expect(r.ok(), await r.text()).toBe(true);
  return (await r.json()).data;
}
async function fixture(page, count) {
  await signIn(page);
  const day = new Date(Date.now() + 86400000).toISOString().slice(0, 10);
  const load = await post(page, "loads", {
    reference: `HISTORY-${Date.now()}-${randomUUID().slice(0, 6)}`,
    customer_id: "c1",
    mode: "FTL",
    equipment: "Flatbed",
    commodity: "Owned history fixture",
    weight_lbs: 1000,
    miles: 100,
    buy_cents: 100000,
    sell_cents: 125000,
    origin: {
      facility: "History pickup dock",
      city: "Portland",
      state: "OR",
      date: day,
      address_lines: ["15 Dock Street"],
      postal_code: "97201",
      country: "US",
    },
    destination: {
      facility: "History receiving dock",
      city: "Seattle",
      state: "WA",
      date: day,
      address_lines: ["25 Dock Street"],
      postal_code: "98101",
      country: "US",
    },
  });
  const book = async (reference) => {
    const current = (await raw(page)).loads.find((l) => l.id === load.id);
    return post(page, `loads/${load.id}/appointments`, {
      expected_load_version: current.version,
      stop: "pickup",
      start_local: `${day}T09:00`,
      end_local: `${day}T10:00`,
      timezone: "UTC",
      contact: "Owned history facility",
      confirmation_reference: reference,
      note: "Original facility authorization retained",
    });
  };
  for (let i = 0; i < count; i++) await book(`OWNED-WINDOW-${i}`);
  return { load, book };
}
test.beforeEach(() =>
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned history and credential fixtures",
  ),
);

test("compact workspace defers original history and pages every appointment without a full refresh payload", async ({
  page,
}) => {
  test.setTimeout(90000);
  await page.goto("/");
  const f = await fixture(page, 30),
    uiReads = [],
    historyReads = [];
  page.on("request", (r) => {
    const u = new URL(r.url());
    if (u.pathname === "/api/freight/workspace")
      uiReads.push(u.searchParams.get("history"));
    if (u.pathname === "/api/freight/history")
      historyReads.push(u.searchParams.get("kind"));
  });
  await page.goto(`/?load=${f.load.id}`);
  const dialog = page.getByRole("dialog");
  await expect(dialog).toContainText(f.load.reference);
  expect(historyReads).not.toContain("appointments");
  expect(historyReads).not.toContain("gates");
  await dialog
    .getByText("Appointment and gate evidence history", { exact: true })
    .click();
  const history = dialog.getByRole("region", {
    name: "Appointment history",
    exact: true,
  });
  await expect(history).toContainText("25 of 30 records");
  await expect(history).toContainText("OWNED-WINDOW-29");
  await expect(history).not.toContainText("OWNED-WINDOW-0 ·");
  await history
    .getByRole("button", { name: "Load older records", exact: true })
    .click();
  await expect(history).toContainText("30 of 30 records");
  await expect(history.locator("article")).toHaveCount(30);
  await expect(history).toContainText("OWNED-WINDOW-0 ·");
  expect(uiReads.length).toBeGreaterThan(0);
  expect(uiReads.every((mode) => mode === "deferred")).toBe(true);
  const response = await page.request.get(
      "/api/freight/workspace?history=deferred",
    ),
    compact = await response.json();
  expect(compact.history.mode).toBe("deferred");
  for (const name of ["events", "appointments", "stop_events"])
    expect(compact[name]).toBeUndefined();
  const original = await raw(page);
  const newest = original.appointments
    .filter((a) => a.load_id === f.load.id)
    .at(-1);
  const facilityTime = await page.evaluate(
    (record) =>
      new Date(record.starts_at).toLocaleString(undefined, {
        timeZone: record.timezone,
        timeZoneName: "short",
      }),
    newest,
  );
  await expect(history).toContainText(facilityTime);
  expect(
    original.appointments.filter((a) => a.load_id === f.load.id),
  ).toHaveLength(30);
  await page.setViewportSize({ width: 390, height: 844 });
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
});

test("a changed shipment keeps loaded history visible and requires an explicit fresh history view", async ({
  page,
}) => {
  test.setTimeout(90000);
  await page.goto("/");
  const f = await fixture(page, 26);
  await page.goto(`/?load=${f.load.id}`);
  const dialog = page.getByRole("dialog");
  await dialog
    .getByText("Appointment and gate evidence history", { exact: true })
    .click();
  const history = dialog.getByRole("region", {
    name: "Appointment history",
    exact: true,
  });
  await expect(history).toContainText("25 of 26 records");
  await f.book("OWNED-LATER-WINDOW");
  await history
    .getByRole("button", { name: "Load older records", exact: true })
    .click();
  await expect(history.getByRole("alert")).toContainText("History changed");
  await expect(history.locator("article")).toHaveCount(25);
  await expect(
    history.getByRole("button", { name: "Load older records", exact: true }),
  ).toBeDisabled();
  await page.route(
    "**/api/freight/workspace?history=deferred",
    (route) =>
      route.fulfill({
        status: 503,
        contentType: "application/json",
        body: JSON.stringify({ error: { message: "Owned refresh failure" } }),
      }),
    { times: 1 },
  );
  await history
    .getByRole("button", { name: "Refresh history", exact: true })
    .click();
  await expect(history.getByRole("alert")).toContainText(
    "Shipment could not be refreshed",
  );
  await expect(history.locator("article")).toHaveCount(25);
  await expect(
    dialog.getByRole("article", { name: "Pickup stop", exact: true }),
  ).not.toContainText("OWNED-LATER-WINDOW");
  await history
    .getByRole("button", { name: "Retry history", exact: true })
    .click();
  await expect(history).toContainText("25 of 27 records");
  await expect(history).toContainText("OWNED-LATER-WINDOW");
  await expect(
    dialog.getByRole("article", { name: "Pickup stop", exact: true }),
  ).toContainText("OWNED-LATER-WINDOW");
  await expect(history.getByRole("alert")).toHaveCount(0);
});

test("history GET verifies the reviewed credential owner before reading another tenant", async ({
  page,
}) => {
  await page.goto("/");
  const f = await fixture(page, 1);
  await page.goto(`/?load=${f.load.id}`);
  const dialog = page.getByRole("dialog");
  await expect(dialog).toContainText(f.load.reference);
  await signIn(page, other);
  const denied = page.waitForResponse(
    (r) =>
      new URL(r.url()).pathname === "/api/freight/history" &&
      r.status() === 409,
  );
  await dialog
    .getByText("Appointment and gate evidence history", { exact: true })
    .click();
  const response = await denied;
  expect((await response.json()).error.code).toBe("workspace_session_changed");
  const reviewed = JSON.parse(
    decodeURIComponent(
      response.request().headers()["x-freight-expected-access"],
    ),
  );
  expect(reviewed.tenant_id).toBe("demo");
  await expect(page.getByLabel("Workspace credential")).toBeVisible();
  await expect(page.getByRole("dialog")).toHaveCount(0);
});

for (const delayedStatus of [200, 409])
  test(`delayed original history ${delayedStatus} cannot change the next signed-in workspace`, async ({
    page,
  }) => {
    await page.goto("/");
    await signIn(page, other);
    await page.reload();
    // Commit the initial verified workspace before changing its cookie. The
    // denial under test must be the held history read, not initial hydration.
    await expect(page.locator(".workspace-switch")).toContainText(
      "quote-source",
    );
    await expect(
      page.getByRole("heading", { name: "Overview", exact: true }),
    ).toBeVisible();
    // A changed cookie makes the held old-owner read a real gateway denial.
    if (delayedStatus === 409) await signIn(page, operator);
    let seen;
    const started = new Promise((resolve) => {
      seen = resolve;
    });
    await page.route(
      "**/api/freight/history?*",
      async (route) => {
        const response = await route.fetch();
        expect(response.status()).toBe(delayedStatus);
        seen({ route, response });
      },
      { times: 1 },
    );
    await page
      .getByRole("navigation")
      .getByRole("button", { name: "Activity log", exact: true })
      .click();
    const older = await started;
    await page
      .getByRole("button", { name: /Operations team.*Sign out/ })
      .click();
    await expect(page.getByLabel("Workspace credential")).toBeVisible();
    await page.getByLabel("Workspace credential").fill(operator);
    await page
      .getByRole("button", { name: "Open workspace", exact: true })
      .click();
    await expect(page.locator(".workspace-switch")).toContainText("demo");
    const next = page.waitForResponse(
      (r) =>
        new URL(r.url()).pathname === "/api/freight/history" &&
        r.status() === 200,
    );
    await page
      .getByRole("navigation")
      .getByRole("button", { name: "Activity log", exact: true })
      .click();
    const nextResponse = await next;
    const nextPage = await nextResponse.json();
    expect(nextPage.tenant_id).toBe("demo");
    expect(nextPage.items.length).toBeGreaterThan(0);
    await nextResponse.finished();
    const history = page.getByRole("region", {
      name: "Workspace history",
      exact: true,
    });
    await expect(history.getByRole("status")).toHaveCount(0);
    await expect(history.locator("article")).toHaveCount(nextPage.items.length);
    const currentText = await history.innerText();
    const late = page.waitForResponse(
      (r) =>
        new URL(r.url()).pathname === "/api/freight/history" &&
        r.status() === delayedStatus,
    );
    await older.route.fulfill({ response: older.response });
    await (await late).finished();
    await expect(page.locator(".workspace-switch")).toContainText("demo");
    await expect.poll(() => history.innerText()).toBe(currentText);
    await expect(history.locator("article")).toHaveCount(nextPage.items.length);
    await expect(page.locator(".workspace-switch")).toContainText("demo");
  });
