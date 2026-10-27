const { test, expect } = require("@playwright/test");
const { randomUUID } = require("node:crypto");
const { writeFile } = require("node:fs/promises");
const credential = "freight-test-operator-credential-at-least-32-chars";
const changedMessage = "This shipment changed while this action was open";

test.beforeEach(async ({ page }) => {
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned synthetic fixtures",
  );
  await page.clock.install();
  await page.goto("/");
  await page.getByLabel("Workspace credential").fill(credential);
  await page
    .getByRole("button", { name: "Open workspace", exact: true })
    .click();
  await expect(
    page.getByRole("heading", { name: "Overview", exact: true }),
  ).toBeVisible();
});

async function setup(page, other = false) {
  const prefix = `REVIEW-${randomUUID().slice(0, 8)}`;
  const origin = new URL(page.url()).origin;
  const post = (path, data) =>
    page.request.post(`/api/freight/${path}`, {
      headers: { Origin: origin, "Idempotency-Key": randomUUID() },
      data,
    });
  const commit = async (path, data) => {
    const response = await post(path, data);
    expect(response.ok(), await response.text()).toBe(true);
    return (await response.json()).data;
  };
  const date = new Date(Date.now() + 86400000).toISOString().slice(0, 10);
  const plan = {
    reference: prefix,
    customer_id: "c1",
    mode: "FTL",
    equipment: "Dry van",
    commodity: "Owned concurrent shipment review",
    weight_lbs: 1000,
    miles: 173,
    buy_cents: 125001,
    sell_cents: 150001,
    origin: {
      facility: "Owned pickup",
      city: "Portland",
      state: "OR",
      date,
      address_lines: ["10 Synthetic Review Street"],
      postal_code: "97201",
      country: "US",
    },
    destination: {
      facility: "Owned delivery",
      city: "Seattle",
      state: "WA",
      date,
      address_lines: ["20 Synthetic Review Street"],
      postal_code: "98101",
      country: "US",
    },
  };
  const created = await commit("loads", plan);
  const second = other
    ? await commit("loads", { ...plan, reference: `${prefix}-OTHER` })
    : null;
  const workspace = async () => {
    const response = await page.request.get("/api/freight/workspace");
    expect(response.ok()).toBe(true);
    return response.json();
  };
  const originalFields = (load) =>
    Object.fromEntries(Object.keys(created).map((key) => [key, load[key]]));
  await page.reload();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /Load board/ })
    .click();
  await page.getByLabel("Search loads").fill(prefix);
  await page.getByRole("button", { name: prefix, exact: true }).click();
  await expect(page.getByRole("dialog")).toContainText(prefix);
  const attempts = [];
  page.on("request", (request) => {
    if (
      request.method() === "POST" &&
      new URL(request.url()).pathname ===
        `/api/freight/loads/${created.id}/commands`
    ) {
      attempts.push(request.postDataJSON());
    }
  });
  const refresh = async () => {
    const ready = page.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === "/api/freight/workspace",
    );
    await page.clock.fastForward(30000);
    await (await ready).finished();
  };
  return {
    prefix,
    plan,
    created,
    second,
    commit,
    workspace,
    originalFields,
    attempts,
    refresh,
  };
}

test("a refreshed shipment revision cannot silently authorize an open tender confirmation", async ({
  page,
}, testInfo) => {
  const owned = await setup(page);
  let dialog = page.getByRole("dialog");
  await dialog.getByRole("button", { name: "Tender", exact: true }).click();
  await dialog.getByLabel("Carrier", { exact: true }).selectOption("ca4");
  const revised = await owned.commit(`loads/${owned.created.id}/plan`, {
    expected_version: owned.created.version,
    plan: {
      ...owned.plan,
      reference: `${owned.prefix}-R`,
      buy_cents: 145003,
      destination: {
        ...owned.plan.destination,
        facility: "Revised receiving dock",
      },
    },
  });
  await owned.refresh();
  await expect(page.locator("tbody")).toContainText(revised.reference);
  const confirm = dialog.getByRole("button", {
    name: "Confirm tender",
    exact: true,
  });
  // Exercise an enabled stale confirmation too, so the preceding defect retains
  // its actual outgoing request and server-side effects rather than just UI text.
  let responseStatus = null;
  if (await confirm.isEnabled()) {
    const response = page.waitForResponse(
      (r) =>
        r.request().method() === "POST" &&
        new URL(r.url()).pathname ===
          `/api/freight/loads/${owned.created.id}/commands`,
    );
    await confirm.click();
    responseStatus = (await response).status();
  }
  let workspace = await owned.workspace();
  const after = workspace.loads.find((load) => load.id === owned.created.id);
  await writeFile(
    testInfo.outputPath("reviewed-tender.json"),
    JSON.stringify(
      {
        reviewed: owned.created,
        independently_revised: revised,
        attempts: owned.attempts,
        response_status: responseStatus,
        after: owned.originalFields(after),
        outbound_jobs: workspace.outbox.filter(
          (job) => job.load_id === owned.created.id,
        ),
      },
      null,
      2,
    ),
  );
  expect(owned.attempts).toEqual([]);
  expect(owned.originalFields(after)).toEqual(revised);
  await expect(
    dialog.getByRole("alert").filter({ hasText: changedMessage }),
  ).toBeVisible();
  await expect(confirm).toBeDisabled();
  await expect(dialog.getByLabel("Carrier", { exact: true })).toHaveValue(
    "ca4",
  );
  // Explicitly close, review the newer shipment and start a fresh decision.
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await page
    .getByRole("button", { name: revised.reference, exact: true })
    .click();
  dialog = page.getByRole("dialog");
  await expect(dialog).toContainText("Revised receiving dock");
  await dialog.getByRole("button", { name: "Tender", exact: true }).click();
  await dialog.getByLabel("Carrier", { exact: true }).selectOption("ca4");
  await dialog
    .getByRole("button", { name: "Confirm tender", exact: true })
    .click();
  await expect(dialog).toHaveCount(0);
  expect(owned.attempts).toEqual([
    { action: "tender", carrier_id: "ca4", expected_version: revised.version },
  ]);
  workspace = await owned.workspace();
  const tendered = workspace.loads.find((load) => load.id === revised.id);
  expect(tendered.status).toBe("tendered");
  expect(tendered.buy_cents).toBe(145003);
  expect(tendered.destination).toEqual(revised.destination);
  expect(
    workspace.outbox.filter(
      (job) => job.load_id === revised.id && job.destination === "stateset-edi",
    ),
  ).toHaveLength(1);
});

test("a server-side command conflict refreshes the source while retaining the original unsaved note", async ({
  page,
}, testInfo) => {
  const owned = await setup(page);
  const dialog = page.getByRole("dialog");
  await dialog
    .getByRole("button", { name: "Flag exception", exact: true })
    .click();
  const note =
    "My unsubmitted facility findings must survive a concurrent report";
  await dialog.getByLabel("Exception details").fill(note);
  const concurrent = await owned.commit(`loads/${owned.created.id}/commands`, {
    action: "exception",
    expected_version: owned.created.version,
    note: "Original independent report retained by the server",
  });
  const response = page.waitForResponse(
    (r) =>
      r.request().method() === "POST" &&
      new URL(r.url()).pathname ===
        `/api/freight/loads/${owned.created.id}/commands`,
  );
  await dialog
    .getByRole("button", { name: "Confirm exception", exact: true })
    .click();
  expect((await response).status()).toBe(409);
  await expect(
    dialog.getByRole("alert").filter({ hasText: changedMessage }),
  ).toBeVisible();
  await expect(dialog.getByLabel("Exception details")).toHaveValue(note);
  await expect(
    dialog.getByRole("button", { name: "Confirm exception", exact: true }),
  ).toBeDisabled();
  expect(owned.attempts).toEqual([
    { action: "exception", note, expected_version: owned.created.version },
  ]);
  const workspace = await owned.workspace();
  const after = owned.originalFields(
    workspace.loads.find((load) => load.id === concurrent.id),
  );
  expect(after).toEqual(concurrent);
  await writeFile(
    testInfo.outputPath("conflicting-exception.json"),
    JSON.stringify(
      { reviewed: owned.created, concurrent, after, attempts: owned.attempts },
      null,
      2,
    ),
  );
});

test("an unrelated shipment refresh preserves a valid confirmation and submits its reviewed version once", async ({
  page,
}, testInfo) => {
  const owned = await setup(page, true);
  const dialog = page.getByRole("dialog");
  await dialog
    .getByRole("button", { name: "Flag exception", exact: true })
    .click();
  const note = "Current verified findings on the reviewed shipment";
  await dialog.getByLabel("Exception details").fill(note);
  const other = await owned.commit(`loads/${owned.second.id}/commands`, {
    action: "exception",
    expected_version: owned.second.version,
    note: "Independent issue on a different shipment",
  });
  await owned.refresh();
  await expect(
    page
      .getByRole("row", { includeHidden: true })
      .filter({ hasText: owned.second.reference }),
  ).toContainText("Needs attention");
  await expect(
    dialog.getByRole("alert").filter({ hasText: changedMessage }),
  ).toHaveCount(0);
  await expect(dialog.getByLabel("Exception details")).toHaveValue(note);
  const confirm = dialog.getByRole("button", {
    name: "Confirm exception",
    exact: true,
  });
  await expect(confirm).toBeEnabled();
  await confirm.click();
  await expect(dialog).toHaveCount(0);
  expect(owned.attempts).toEqual([
    { action: "exception", note, expected_version: owned.created.version },
  ]);
  const workspace = await owned.workspace();
  const changed = workspace.loads.find((load) => load.id === owned.created.id);
  expect(changed.exception).toBe(note);
  expect(changed.version).toBe(owned.created.version + 1);
  expect(
    owned.originalFields(workspace.loads.find((load) => load.id === other.id)),
  ).toEqual(other);
  await writeFile(
    testInfo.outputPath("unrelated-refresh.json"),
    JSON.stringify(
      {
        reviewed: owned.created,
        independently_changed: other,
        after: owned.originalFields(changed),
        attempts: owned.attempts,
      },
      null,
      2,
    ),
  );
});
