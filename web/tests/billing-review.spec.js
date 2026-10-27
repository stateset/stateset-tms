const { staffOrigin, apiOrigin } = require("./service-origins.cjs")();
const { test, expect } = require("@playwright/test");
const { randomUUID } = require("node:crypto");
const { writeFile } = require("node:fs/promises");
const operator = "freight-test-operator-credential-at-least-32-chars";
const finance = "freight-test-finance-credential-at-least-32-chars";
const other = "freight-recovery-test-credential-at-least-32-chars";
const changed = "The billing identity changed while this form was open";
const profilePath = "/api/freight/finance/profile";
test.beforeEach(async ({ page }) => {
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned synthetic identities and workspaces",
  );
  await page.clock.install();
});
async function signIn(page, token) {
  const r = await page.request.post("/api/session", {
    headers: { Origin: staffOrigin },
    data: { token },
  });
  expect(r.ok()).toBe(true);
}
async function snapshot(page, token = operator) {
  const r = await page.request.get(`${apiOrigin}/v1/workspace`, {
    headers: { Authorization: `Bearer ${token}` },
  });
  expect(r.ok()).toBe(true);
  return r.json();
}
async function commit(page, path, body, token = finance) {
  const r = await page.request.post(`${apiOrigin}/v1/${path}`, {
    headers: {
      Authorization: `Bearer ${token}`,
      "Idempotency-Key": randomUUID(),
    },
    data: body,
  });
  expect(r.ok(), await r.text()).toBe(true);
  return (await r.json()).data;
}
const dialogFor = (page) =>
  page.getByRole("dialog", { name: "Billing issuer identity", exact: true });
async function edit(page) {
  await page
    .getByRole("button", { name: "Edit billing identity", exact: true })
    .click();
  await expect(dialogFor(page)).toBeVisible();
  return dialogFor(page);
}
async function fields(dialog) {
  return {
    legal_name: await dialog
      .getByLabel("Issuing legal name", { exact: true })
      .inputValue(),
    address: await dialog
      .getByLabel("Issuing address", { exact: true })
      .inputValue(),
    contact: await dialog
      .getByLabel("Billing contact", { exact: true })
      .inputValue(),
    payment_instructions: await dialog
      .getByLabel("Payment instructions", { exact: true })
      .inputValue(),
  };
}
const save = (dialog) =>
  dialog
    .getByRole("button", { name: "Save billing identity", exact: true })
    .click();
async function refresh(page, name) {
  const ready = page.waitForResponse(
    (r) => new URL(r.url()).pathname === "/api/freight/workspace",
  );
  await page.clock.fastForward(30000);
  await (await ready).finished();
  if (name) await expect(page.locator(".billing-identity")).toContainText(name);
}
async function setup(page, { token = operator, blank = false } = {}) {
  const before = await snapshot(page, token),
    name = `Owned billing review ${randomUUID().slice(0, 8)}`,
    profile = {
      legal_name: name,
      address: "Owned issuing address",
      contact: "owned-billing@example.test",
      payment_instructions: "Owned original payment instructions",
    };
  const initial = blank
    ? null
    : await commit(
        page,
        "finance/profile",
        { ...profile, expected_version: before.billing_profile?.version || 0 },
        token === operator ? finance : token,
      );
  if (blank) expect(before.billing_profile).toBeNull();
  await signIn(page, token);
  await page.goto("/");
  await expect(
    page.getByRole("heading", { name: "Overview", exact: true }),
  ).toBeVisible();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Freight audit", exact: true })
    .click();
  await page
    .getByRole("tab", { name: "Customer billing", exact: true })
    .click();
  const dialog = await edit(page),
    posts = [];
  page.on("request", (r) => {
    if (r.method() === "POST" && new URL(r.url()).pathname === profilePath)
      posts.push({
        key: r.headers()["idempotency-key"],
        body: r.postDataJSON(),
        reviewed_access: r.headers()["x-freight-expected-access"],
      });
  });
  const concurrent = async (basis = initial, extra = {}) =>
    commit(
      page,
      "finance/profile",
      {
        ...profile,
        legal_name: name + " concurrent",
        payment_instructions: "Owned updated payment instructions",
        expected_version: basis?.version || 0,
        ...extra,
      },
      token === operator ? finance : token,
    );
  const record = async (info, filename, extra = {}) => {
    const current = await snapshot(page, token),
      old = new Set(before.events.map((e) => e.id));
    await writeFile(
      info.outputPath(filename),
      JSON.stringify(
        {
          name,
          tenant: before.tenant_id,
          initial,
          posts,
          final: current.billing_profile,
          profile_events: current.events.filter(
            (e) => e.kind === "bill.profile" && !old.has(e.id),
          ),
          ...extra,
        },
        null,
        2,
      ),
    );
  };
  return {
    before,
    name,
    profile,
    initial,
    dialog,
    posts,
    concurrent,
    record,
    token,
  };
}

test("a first-time identity form cannot overwrite an independently created issuer after refresh", async ({
  page,
}, info) => {
  const s = await setup(page, { token: other, blank: true });
  for (const [label, value] of [
    ["Issuing legal name", s.profile.legal_name],
    ["Issuing address", s.profile.address],
    ["Billing contact", s.profile.contact],
    ["Payment instructions", s.profile.payment_instructions],
  ])
    await s.dialog.getByLabel(label, { exact: true }).fill(value);
  const typed = await fields(s.dialog),
    concurrent = await s.concurrent();
  await refresh(page, concurrent.legal_name);
  await expect(s.dialog.getByRole("alert")).toContainText(changed);
  expect(await fields(s.dialog)).toEqual(typed);
  await save(s.dialog);
  expect(s.posts).toEqual([]);
  expect((await snapshot(page, other)).billing_profile).toEqual(concurrent);
  await s.record(info, "billing-first-identity.json", { concurrent, typed });
});

test("a refreshed billing identity keeps the original review and requires explicit reopening before a new save", async ({
  page,
}, info) => {
  const s = await setup(page);
  await s.dialog
    .getByLabel("Issuing legal name", { exact: true })
    .fill(s.name + " draft");
  const typed = await fields(s.dialog),
    concurrent = await s.concurrent();
  await refresh(page, concurrent.legal_name);
  await expect(s.dialog.getByRole("alert")).toContainText(changed);
  expect(await fields(s.dialog)).toEqual(typed);
  await save(s.dialog);
  expect(s.posts).toEqual([]);
  const blocked = (await snapshot(page)).billing_profile;
  expect(blocked).toEqual(concurrent);
  await s.dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  const reopened = await edit(page);
  expect(await fields(reopened)).toEqual(
    Object.fromEntries(
      Object.entries(concurrent).filter(([k]) => k !== "version"),
    ),
  );
  await reopened
    .getByLabel("Issuing legal name", { exact: true })
    .fill(s.name + " reviewed");
  await save(reopened);
  await expect(reopened).toHaveCount(0);
  const final = (await snapshot(page)).billing_profile;
  expect(final).toEqual({
    ...concurrent,
    legal_name: s.name + " reviewed",
    version: concurrent.version + 1,
  });
  expect(s.posts).toHaveLength(1);
  expect(s.posts[0].body.expected_version).toBe(concurrent.version);
  expect(JSON.parse(decodeURIComponent(s.posts[0].reviewed_access))).toEqual({
    tenant_id: s.before.tenant_id,
    subject: s.before.access.subject,
  });
  await s.record(info, "billing-refreshed-review.json", {
    concurrent,
    typed,
    blocked,
  });
});

test("an unseen issuer change returns a real version conflict and refreshes without replacing unsaved fields", async ({
  page,
}, info) => {
  const s = await setup(page);
  await s.dialog
    .getByLabel("Issuing address", { exact: true })
    .fill("Owned unsaved replacement address");
  const typed = await fields(s.dialog),
    concurrent = await s.concurrent(),
    response = page.waitForResponse(
      (r) => new URL(r.url()).pathname === profilePath,
    );
  await save(s.dialog);
  const rejected = await response;
  expect(rejected.status()).toBe(409);
  await expect(s.dialog.getByRole("alert")).toContainText(changed);
  expect(await fields(s.dialog)).toEqual(typed);
  expect(s.posts).toHaveLength(1);
  expect(s.posts[0].body.expected_version).toBe(s.initial.version);
  expect((await snapshot(page)).billing_profile).toEqual(concurrent);
  await s.record(info, "billing-unseen-conflict.json", {
    concurrent,
    typed,
    rejected_status: rejected.status(),
  });
});

test("unrelated workspace refreshes leave a reviewed billing identity editable", async ({
  page,
}, info) => {
  const s = await setup(page);
  await s.dialog
    .getByLabel("Billing contact", { exact: true })
    .fill("owned-revised@example.test");
  const typed = await fields(s.dialog),
    customer = await commit(
      page,
      "customers",
      {
        name: s.name + " unrelated",
        contact: "owned-customer@example.test",
        billing_address: "Owned unrelated address",
      },
      operator,
    );
  await refresh(page, s.initial.legal_name);
  await expect(s.dialog.getByRole("alert")).toHaveCount(0);
  expect(await fields(s.dialog)).toEqual(typed);
  await save(s.dialog);
  await expect(s.dialog).toHaveCount(0);
  expect(s.posts).toHaveLength(1);
  expect(s.posts[0].body.expected_version).toBe(s.initial.version);
  expect((await snapshot(page)).billing_profile).toEqual({
    ...typed,
    version: s.initial.version + 1,
  });
  await s.record(info, "billing-unrelated-refresh.json", { typed, customer });
});

test("an unchanged lost-response retry returns its original receipt after another finance operator updates the issuer", async ({
  page,
}, info) => {
  const s = await setup(page);
  await s.dialog
    .getByLabel("Issuing legal name", { exact: true })
    .fill(s.name + " original save");
  let committed;
  await page.route(
    "**/api/freight/finance/profile",
    async (route) => {
      const r = await route.fetch();
      expect(r.ok()).toBe(true);
      committed = (await r.json()).data;
      await route.abort("failed");
    },
    { times: 1 },
  );
  await save(s.dialog);
  await expect(s.dialog.getByRole("alert")).toBeVisible();
  expect(committed.version).toBe(s.initial.version + 1);
  const original = { ...s.posts[0] },
    concurrent = await s.concurrent(committed);
  await refresh(page, concurrent.legal_name);
  await expect(s.dialog.getByRole("alert")).toContainText(changed);
  await s.dialog
    .getByLabel("Billing contact", { exact: true })
    .fill("owned-altered-retry@example.test");
  await save(s.dialog);
  expect(s.posts).toEqual([original]);
  expect((await snapshot(page)).billing_profile).toEqual(concurrent);
  await s.dialog
    .getByLabel("Billing contact", { exact: true })
    .fill(original.body.contact);
  const events = (await snapshot(page)).events.filter(
      (e) => e.kind === "bill.profile",
    ),
    response = page.waitForResponse(
      (r) => new URL(r.url()).pathname === profilePath,
    );
  await save(s.dialog);
  const receipt = await response,
    result = await receipt.json();
  expect(receipt.ok()).toBe(true);
  expect(result.replayed).toBe(true);
  expect(result.data).toEqual(committed);
  await expect(s.dialog).toHaveCount(0);
  expect(s.posts).toEqual([original, original]);
  const final = await snapshot(page);
  expect(final.billing_profile).toEqual(concurrent);
  expect(final.events.filter((e) => e.kind === "bill.profile")).toEqual(events);
  await s.record(info, "billing-lost-response.json", {
    committed,
    concurrent,
    replayed: result,
  });
});

test("failed conflict refreshes retain the old version and edits through retries until a fresh review", async ({
  page,
}, info) => {
  const s = await setup(page);
  await s.dialog
    .getByLabel("Payment instructions", { exact: true })
    .fill("Owned unsaved remittance proposal");
  const typed = await fields(s.dialog),
    concurrent = await s.concurrent();
  await page.route("**/api/freight/workspace?history=deferred", (route) =>
    route.fulfill({
      status: 503,
      contentType: "application/json",
      body: JSON.stringify({
        error: { message: "Owned temporary read outage" },
      }),
    }),
  );
  const rejected = [];
  for (let i = 0; i < 2; i++) {
    const result = page.waitForResponse(
      (r) => new URL(r.url()).pathname === profilePath,
    );
    await save(s.dialog);
    rejected.push((await result).status());
    await expect(
      s.dialog.getByRole("button", {
        name: "Save billing identity",
        exact: true,
      }),
    ).toBeEnabled();
    expect(await fields(s.dialog)).toEqual(typed);
  }
  expect(rejected).toEqual([409, 409]);
  expect(s.posts).toHaveLength(2);
  expect(s.posts[0]).toEqual(s.posts[1]);
  expect(s.posts[0].body.expected_version).toBe(s.initial.version);
  expect((await snapshot(page)).billing_profile).toEqual(concurrent);
  await page.unroute("**/api/freight/workspace?history=deferred");
  await refresh(page, concurrent.legal_name);
  await expect(s.dialog.getByRole("alert")).toContainText(changed);
  expect(await fields(s.dialog)).toEqual(typed);
  await s.record(info, "billing-failed-refresh.json", {
    typed,
    concurrent,
    rejected,
  });
});

for (const [label, token] of [
  ["tenant", other],
  ["operator", finance],
]) {
  test(`a changed credential ${label} cannot receive an already reviewed billing identity write`, async ({
    page,
  }, info) => {
    const s = await setup(page),
      destinationBefore = await snapshot(page, token);
    await s.dialog
      .getByLabel("Issuing legal name", { exact: true })
      .fill(s.name + " scope fence");
    await signIn(page, token);
    const response = page.waitForResponse(
      (r) => new URL(r.url()).pathname === profilePath,
    );
    await save(s.dialog);
    const rejected = await response,
      result = await rejected.json();
    expect(rejected.status()).toBe(409);
    expect(result.error.code).toBe("workspace_session_changed");
    await expect(page.getByLabel("Workspace credential")).toBeVisible();
    expect((await snapshot(page)).billing_profile).toEqual(s.initial);
    expect((await snapshot(page, token)).billing_profile).toEqual(
      destinationBefore.billing_profile,
    );
    expect(s.posts).toHaveLength(1);
    expect(s.posts[0].reviewed_access).toBeTruthy();
    await s.record(info, `billing-identity-${label}.json`, {
      destination_tenant: destinationBefore.tenant_id,
      destination_profile: destinationBefore.billing_profile,
      rejected_status: rejected.status(),
      rejected_body: result,
    });
    if (label === "operator")
      await writeFile(
        info.outputPath("billing-review-complete.json"),
        JSON.stringify(
          {
            expected_profiles: {
              demo: (await snapshot(page)).billing_profile,
              recovery: (await snapshot(page, other)).billing_profile,
            },
          },
          null,
          2,
        ),
      );
  });
}
