const { test, expect } = require("@playwright/test");
const { randomUUID, createHash } = require("node:crypto");

test("control tower owns real issues, retains source-verified recovery and recurrence, and protects mobile staff and partner access", async ({
  page,
  browser,
}) => {
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned credential services",
  );
  test.setTimeout(90000);
  page.setDefaultTimeout(12000);
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.clock.install();
  await page.goto("/");
  await page
    .getByLabel("Workspace credential")
    .fill("freight-test-operator-credential-at-least-32-chars");
  await page
    .getByRole("button", { name: "Open workspace", exact: true })
    .click();
  await expect(
    page.getByRole("heading", { name: "Overview", exact: true }),
  ).toBeVisible();
  const origin = new URL(page.url()).origin,
    reference = `TOWER-${Date.now()}`,
    day = new Date(Date.now() + 86400000).toISOString().slice(0, 10),
    future = (minutes) =>
      new Date(Date.now() + minutes * 60000).toISOString().slice(0, 16),
    actor =
      "credential:" +
      createHash("sha256")
        .update("freight-test-operator-credential-at-least-32-chars")
        .digest("hex")
        .slice(0, 12);
  const post = async (path, data, context = page.request) => {
    const r = await context.post(`/api/freight/${path}`, {
      headers: { Origin: origin, "Idempotency-Key": randomUUID() },
      data,
    });
    expect(r.ok(), await r.text()).toBe(true);
    return (await r.json()).data;
  };
  const workspace = async () =>
    (await page.request.get("/api/freight/workspace")).json();
  const stop = (facility, city, state, postal_code) => ({
    facility,
    city,
    state,
    postal_code,
    country: "US",
    date: day,
    address_lines: ["10 Synthetic Operations Street"],
  });
  const created = await post("loads", {
    reference,
    customer_id: "c1",
    mode: "FTL",
    equipment: "Dry van",
    commodity: "Control tower qualification",
    weight_lbs: 1000,
    miles: 173,
    buy_cents: 125001,
    sell_cents: 150001,
    origin: stop("Portland tower origin", "Portland", "OR", "97201"),
    destination: stop("Seattle tower receiver", "Seattle", "WA", "98101"),
  });
  await post(`loads/${created.id}/commands`, {
    action: "exception",
    expected_version: created.version,
    note: "Actual facility closure requires carrier coordination",
  });
  const before = (await workspace()).loads.find((l) => l.id === created.id);
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Control tower", exact: true })
    .click();
  const tower = page.getByRole("region", {
      name: "Operations control tower",
      exact: true,
    }),
    search = tower.getByLabel("Search operations issues"),
    view = tower.getByLabel("Operations queue view"),
    card = tower.getByRole("article", {
      name: `Shipment exception · ${reference}`,
      exact: true,
    });
  await search.fill(reference);
  await expect(card).toBeVisible();
  await expect(card).toContainText("Unassigned");
  const fill = async (name, note, due) => {
    await card.getByRole("button", { name, exact: true }).click();
    const d = page.getByRole("dialog");
    if (due) await d.getByLabel("Next follow-up (UTC)").fill(future(due));
    await d.getByLabel("Follow-up findings and next step").fill(note);
    await d.getByRole("checkbox").check();
    return d;
  };
  let d = await fill(
    "Take ownership",
    "PRIVATE original owner will contact the actual facility and carrier",
    90,
  );
  await d
    .getByRole("button", { name: "Record follow-up action", exact: true })
    .click();
  await expect(d).toHaveCount(0);
  await expect(card).toContainText(actor);
  let queue = await (
      await page.request.get(
        `/api/freight/operations?search=${reference}&view=active`,
      )
    ).json(),
    row = queue.issues[0];
  const issueId = row.id;
  expect(queue.total).toBe(1);
  expect(row.load_reference).toBe(reference);
  expect(row.owner).toBe(actor);
  expect(row.case.history).toBeUndefined();
  expect((await workspace()).loads.find((l) => l.id === created.id)).toEqual(
    before,
  );
  await view.selectOption("mine");
  await expect(card).toBeVisible();
  d = await fill(
    "Defer follow-up",
    "PRIVATE waiting for the actual facility callback within the agreed deadline",
  );
  await d.getByLabel("Defer until (UTC)").fill(future(15));
  await d
    .getByRole("button", { name: "Record follow-up action", exact: true })
    .click();
  await expect(d).toHaveCount(0);
  await expect(card).toContainText("Deferred until");
  await view.selectOption("deferred");
  await expect(card).toBeVisible();
  expect(
    (await workspace()).loads.find((l) => l.id === created.id).exception,
  ).toBe(before.exception);
  d = await fill(
    "Resume follow-up",
    "PRIVATE actual facility answered; resume handling now",
  );
  await d
    .getByRole("button", { name: "Record follow-up action", exact: true })
    .click();
  await expect(d).toHaveCount(0);
  await expect(card).toHaveCount(0);
  await view.selectOption("active");
  await expect(card).toBeVisible();
  await expect(
    card.getByRole("button", { name: "Update follow-up", exact: true }),
  ).toBeEnabled();
  let releaseRefresh, sawRefresh;
  const refreshReleased = new Promise((resolve) => {
      releaseRefresh = resolve;
    }),
    refreshStarted = new Promise((resolve) => {
      sawRefresh = resolve;
    });
  let holdRefresh = true;
  const heldRefresh = async (route) => {
    if (holdRefresh) {
      sawRefresh();
      await refreshReleased;
    }
    await route.continue();
  };
  await page.route("**/api/freight/operations?**", heldRefresh);
  try {
    await page.clock.fastForward(30000);
    await refreshStarted;
    // A background list read must not cancel a user's follow-up click. The
    // form fetches fresh detail; its later write still binds that reviewed source.
    await expect(
      card.getByRole("button", { name: "Update follow-up", exact: true }),
    ).toBeEnabled();
    d = await fill(
      "Update follow-up",
      "PRIVATE stale review must not be committed",
      120,
    );
  } finally {
    holdRefresh = false;
    releaseRefresh();
    await page.unrouteAll({ behavior: "wait" });
  }
  await expect(d.getByLabel("Follow-up findings and next step")).toHaveValue(
    "PRIVATE stale review must not be committed",
  );
  const overloadedRefresh = async (route) => {
    await route.fulfill({
      status: 503,
      contentType: "application/json",
      headers: { "Retry-After": "1", "Cache-Control": "no-store" },
      body: JSON.stringify({
        error: {
          code: "read_capacity_exceeded",
          message: "Workspace is busy. Please retry shortly.",
        },
      }),
    });
  };
  await page.route("**/api/freight/operations?**", overloadedRefresh);
  try {
    await page.clock.fastForward(30000);
    await expect(
      page.getByText("Workspace is busy. Please retry shortly.", {
        exact: true,
      }),
    ).toBeVisible();
    await expect(
      tower.getByRole("article", {
        name: `Shipment exception · ${reference}`,
        exact: true,
        includeHidden: true,
      }),
    ).toBeVisible();
    await expect(d.getByLabel("Follow-up findings and next step")).toHaveValue(
      "PRIVATE stale review must not be committed",
    );
    await expect(
      page.getByRole("button", { name: "Open workspace", exact: true }),
    ).toHaveCount(0);
  } finally {
    await page.unroute("**/api/freight/operations?**", overloadedRefresh);
  }
  row = await (
    await page.request.get(`/api/freight/operations/issues/${issueId}`)
  ).json();
  await post(`operations/issues/${issueId}/actions`, {
    expected_version: row.case.version,
    expected_source_active: row.source_active,
    source_sha256: row.source_sha256,
    action: "note",
    note: "PRIVATE concurrent source review retained",
  });
  const reviewed = await (
    await page.request.get(`/api/freight/operations/issues/${issueId}`)
  ).json();
  await d
    .getByRole("button", { name: "Record follow-up action", exact: true })
    .click();
  await expect(d.getByRole("alert")).toContainText("Issue changed");
  expect(
    (
      await (
        await page.request.get(`/api/freight/operations/issues/${issueId}`)
      ).json()
    ).case.version,
  ).toBe(reviewed.case.version);
  await page.keyboard.press("Escape");
  await tower
    .getByRole("button", { name: "Refresh issues", exact: true })
    .click();
  await page.setViewportSize({ width: 390, height: 844 });
  d = await fill(
    "Update follow-up",
    "PRIVATE actual next carrier call is now scheduled",
    120,
  );
  await d
    .getByRole("button", { name: "Record follow-up action", exact: true })
    .click();
  await expect(d).toHaveCount(0);
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBe(true);
  if (process.env.FREIGHT_E2E_SCREENSHOTS === "true")
    await tower.screenshot({
      path: "../docs/screenshots/control-tower-mobile.png",
    });
  await card
    .getByRole("button", { name: "View issue history", exact: true })
    .click();
  d = page.getByRole("dialog", { name: "Issue history", exact: true });
  await expect(d).toContainText("PRIVATE original owner");
  await expect(d).toContainText("PRIVATE concurrent source review retained");
  await expect(d).not.toContainText(
    "PRIVATE stale review must not be committed",
  );
  await page.keyboard.press("Escape");
  let load = (await workspace()).loads.find((l) => l.id === created.id);
  await post(`loads/${load.id}/commands`, {
    action: "resolve",
    expected_version: load.version,
  });
  await tower
    .getByRole("button", { name: "Refresh issues", exact: true })
    .click();
  await expect(card).toHaveCount(0);
  await view.selectOption("resolved");
  await expect(card).toContainText("Source condition cleared");
  row = await (
    await page.request.get(`/api/freight/operations/issues/${issueId}`)
  ).json();
  expect(row.case.status).toBe("resolved");
  expect(row.case.history.at(-1)).toMatchObject({
    action: "source_cleared",
    actor: "system:operations",
  });
  load = (await workspace()).loads.find((l) => l.id === created.id);
  await post(`loads/${load.id}/commands`, {
    action: "exception",
    expected_version: load.version,
    note: "Actual new closure requires a new response",
  });
  await tower
    .getByRole("button", { name: "Refresh issues", exact: true })
    .click();
  await view.selectOption("active");
  await expect(card).toContainText("Unassigned");
  row = await (
    await page.request.get(`/api/freight/operations/issues/${issueId}`)
  ).json();
  expect(row.case.cycle).toBe(2);
  expect(row.owner).toBeNull();
  expect(row.case.deferred_until).toBeNull();
  const ops = await browser.newContext({ baseURL: origin }),
    viewer = await browser.newContext({
      baseURL: origin,
      viewport: { width: 390, height: 844 },
    }),
    partner = await browser.newContext({ baseURL: origin });
  try {
    for (const [context, role, path] of [
      [ops, "operations", "session"],
      [viewer, "viewer", "session"],
      [partner, "carrier", "portal-session"],
    ]) {
      context.setDefaultTimeout(12000);
      expect(
        (
          await context.request.post(`/api/${path}`, {
            headers: { Origin: origin },
            data: {
              token: `freight-test-${role}-credential-at-least-32-chars`,
            },
          })
        ).ok(),
      ).toBe(true);
    }
    await post(
      `operations/issues/${issueId}/actions`,
      {
        expected_version: row.case.version,
        expected_source_active: row.source_active,
        source_sha256: row.source_sha256,
        action: "claim",
        note: "Actual operations colleague takes responsibility",
        follow_up_at: future(60) + ":00Z",
      },
      ops.request,
    );
    await tower
      .getByRole("button", { name: "Refresh issues", exact: true })
      .click();
    await expect(card).toContainText("test:operations");
    d = await fill(
      "Release another owner",
      "Actual administrator released the unavailable owner after coordination",
    );
    await d
      .getByRole("button", { name: "Record follow-up action", exact: true })
      .click();
    await expect(d).toHaveCount(0);
    await expect(card).toContainText("Unassigned");
    await tower
      .getByRole("button", { name: "Tracking policy", exact: true })
      .click();
    d = page.getByRole("dialog", {
      name: "Tracking follow-up policy",
      exact: true,
    });
    await d.getByLabel("Tracking freshness interval (minutes)").fill("120");
    await d
      .getByLabel("Policy change reason")
      .fill("Actual two-hour operating follow-up policy");
    await d
      .getByRole("button", { name: "Save tracking policy", exact: true })
      .click();
    await expect(d).toHaveCount(0);
    expect((await workspace()).operations.policy.tracking_stale_minutes).toBe(
      120,
    );
    const v = await viewer.newPage();
    v.on("pageerror", (e) => errors.push(e.message));
    await v.goto("/");
    await v
      .getByRole("button", { name: "Toggle navigation", exact: true })
      .click();
    await v
      .getByRole("navigation")
      .getByRole("button", { name: "Control tower", exact: true })
      .click();
    await v.getByLabel("Search operations issues").fill(reference);
    const readonly = v.getByRole("article", {
      name: `Shipment exception · ${reference}`,
      exact: true,
    });
    await expect(
      readonly.getByRole("button", { name: "Take ownership", exact: true }),
    ).toBeDisabled();
    await expect(
      v.getByRole("button", { name: "Tracking policy", exact: true }),
    ).toHaveCount(0);
    expect(
      (
        await viewer.request.post(
          `/api/freight/operations/issues/${issueId}/actions`,
          {
            headers: { Origin: origin, "Idempotency-Key": randomUUID() },
            data: {},
          },
        )
      ).status(),
    ).toBe(403);
    expect(
      (
        await partner.request.get(`/api/freight/operations/issues/${issueId}`)
      ).status(),
    ).toBe(403);
    expect((await partner.request.get("/api/freight/portal")).ok()).toBe(true);
    expect(
      await v.evaluate(
        () => document.documentElement.scrollWidth <= window.innerWidth,
      ),
    ).toBe(true);
    expect(errors).toEqual([]);
  } finally {
    await Promise.allSettled([ops.close(), viewer.close(), partner.close()]);
  }
});
