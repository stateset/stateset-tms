const { clerkOrigin, fixtureOrigin } = require("./service-origins.cjs")();
const { test, expect } = require("@playwright/test");
const { randomUUID } = require("node:crypto");
const { writeFile } = require("node:fs/promises");
const credential = "freight-test-operator-credential-at-least-32-chars";
const other = "freight-test-quote-source-credential-at-least-32-chars";
const recovery = (p) =>
  p.getByRole("region", { name: "Save recovery", exact: true });
const storage = (p) =>
  p.getByRole("region", { name: "Recovery storage", exact: true });
const record = async (info, name, value) =>
  writeFile(info.outputPath(name), JSON.stringify(value, null, 2));
const business = (r) =>
  r.method() === "POST" &&
  r.url().includes("/api/freight/") &&
  !r.url().endsWith("/api/freight/save-journal");
async function login(page) {
  await page.goto("/");
  await page.getByLabel("Workspace credential").fill(credential);
  await page
    .getByRole("button", { name: "Open workspace", exact: true })
    .click();
  await expect(
    page.getByRole("heading", { name: "Overview", exact: true }),
  ).toBeVisible();
}
async function owner(page) {
  const a = await (await page.request.get("/api/freight/access")).json();
  return JSON.stringify(["credential", "", a.tenant_id, a.access.subject, ""]);
}
async function cryptoCall(page, data) {
  const r = await page.request.post("/api/freight/save-journal", {
    headers: { Origin: new URL(page.url()).origin },
    data,
  });
  expect(r.headers()["cache-control"]).toBe("no-store");
  expect(r.headers()["x-content-type-options"]).toBe("nosniff");
  return { status: r.status(), result: await r.json() };
}
async function db(page, action, key, value) {
  return page.evaluate(
    async ({ action, key, value }) => {
      const db = await new Promise((resolve, reject) => {
        const r = indexedDB.open("stateset-freight-save-recovery");
        r.onsuccess = () => resolve(r.result);
        r.onerror = () => reject(r.error);
      });
      try {
        return await new Promise((resolve, reject) => {
          const tx = db.transaction(
              "owners",
              action === "set" ? "readwrite" : "readonly",
            ),
            s = tx.objectStore("owners");
          const r = action === "set" ? s.put(value, key) : s.get(key);
          let v;
          r.onsuccess = () => {
            v = r.result;
          };
          tx.oncomplete = () => resolve(v);
          tx.onabort = () => reject(tx.error);
        });
      } finally {
        db.close();
      }
    },
    { action, key, value },
  );
}
function attempt(name) {
  const body = {
      name,
      contact: `private-${randomUUID()}@example.test`,
      billing_address: "PRIVATE freight billing address",
    },
    path = "customers";
  return {
    key: randomUUID(),
    path,
    body,
    fingerprint: JSON.stringify([path, body]),
    message: "Customer added",
    kind: "unknown",
    checks: 0,
    createdAt: new Date().toISOString(),
  };
}
async function rows(page, name) {
  return (
    await (await page.request.get("/api/freight/workspace")).json()
  ).customers.filter((c) => c.name === name);
}
async function form(page, a) {
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Customers", exact: true })
    .click();
  await page.getByRole("button", { name: "Add customer", exact: true }).click();
  const d = page.getByRole("dialog");
  await d.getByLabel("Customer name", { exact: true }).fill(a.body.name);
  await d.getByLabel("Contact email or phone").fill(a.body.contact);
  return d;
}
async function uncertain(page, a) {
  const d = await form(page, a);
  await page.route("**/api/freight/customers", (r) => r.abort("failed"));
  await d.getByRole("button", { name: "Save record", exact: true }).click();
  await expect(recovery(page)).toBeVisible();
}
async function holdSuccessfulJournalReplies(page, action) {
  let release;
  const held = new Promise((resolve) => {
    release = resolve;
  });
  const pending = new Set();
  const handler = async (route) => {
    if (route.request().postDataJSON().action !== action)
      return route.continue();
    let complete;
    const done = new Promise((resolve) => {
      complete = resolve;
    });
    pending.add(done);
    try {
      const response = await route.fetch();
      if (response.ok()) await held;
      await route.fulfill({ response });
    } finally {
      pending.delete(done);
      complete();
    }
  };
  await page.route("**/api/freight/save-journal", handler);
  return async () => {
    release();
    await Promise.all([...pending]);
    await page.unroute("**/api/freight/save-journal", handler);
  };
}
test.beforeEach(async ({ page }) => {
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned gateway and browser data",
  );
  await page.clock.install();
  await login(page);
});

test("ciphertext retains exact intent while other authenticated tenants and operators cannot open or relabel it", async ({
  page,
}, info) => {
  const a = attempt(`PRIVATE sealed ${randomUUID().slice(0, 8)}`);
  await uncertain(page, a);
  const scope = await owner(page),
    raw = await db(page, "get", scope);
  expect(raw.version).toBe(2);
  expect(Object.keys(raw.attempt).sort()).toEqual([
    "checks",
    "key",
    "kind",
    "sealed",
  ]);
  const serialized = JSON.stringify(raw);
  expect(serialized).not.toContain(a.body.name);
  expect(serialized).not.toContain(a.body.contact);
  expect(serialized).not.toContain("Customer added");
  const opened = await cryptoCall(page, {
    action: "open",
    owner: scope,
    sealed: raw.attempt.sealed,
  });
  expect(opened.status).toBe(200);
  expect(opened.result.data.body.name).toBe(a.body.name);
  const denied = [];
  for (const token of [
    other,
    "freight-test-operations-credential-at-least-32-chars",
  ]) {
    await page.request.post("/api/session", {
      headers: { Origin: new URL(page.url()).origin },
      data: { token },
    });
    const newScope = await owner(page);
    const mismatched = await cryptoCall(page, {
      action: "open",
      owner: scope,
      sealed: raw.attempt.sealed,
    });
    const relabeled = await cryptoCall(page, {
      action: "open",
      owner: newScope,
      sealed: raw.attempt.sealed,
    });
    expect(mismatched.status).toBe(409);
    expect(relabeled.status).toBe(409);
    expect(JSON.stringify([mismatched, relabeled])).not.toContain(a.body.name);
    denied.push({ newScope, mismatched, relabeled });
  }
  await page.request.post("/api/session", {
    headers: { Origin: new URL(page.url()).origin },
    data: { token: credential },
  });
  const posts = [];
  page.on("request", (r) => {
    if (business(r)) posts.push(r.url());
  });
  await page.reload();
  await expect(recovery(page)).toBeVisible();
  expect(posts).toEqual([]);
  await recovery(page)
    .getByText("Original save details", { exact: true })
    .click();
  await expect(recovery(page).getByLabel("Original save values")).toContainText(
    a.body.name,
  );
  await page.unroute("**/api/freight/customers");
  await recovery(page)
    .getByRole("button", { name: "Check original save", exact: true })
    .click();
  await expect(recovery(page)).toHaveCount(0);
  const customers = await rows(page, a.body.name);
  expect(customers).toHaveLength(1);
  await record(info, "sealed-owner-boundary.json", {
    name: a.body.name,
    scope,
    raw,
    opened,
    denied,
    posts,
    customers,
  });
});

test("an authenticated legacy conversion preserves the original key and retrieves its existing committed receipt", async ({
  page,
}, info) => {
  const a = attempt(`Owned legacy committed ${randomUUID().slice(0, 8)}`),
    scope = await owner(page),
    legacy = { version: 1, generation: randomUUID(), attempt: a };
  const original = await page.request.post("/api/freight/customers", {
    headers: { Origin: new URL(page.url()).origin, "Idempotency-Key": a.key },
    data: a.body,
  });
  expect(original.ok()).toBe(true);
  const originalResult = await original.json();
  await db(page, "set", scope, legacy);
  const posts = [];
  page.on("request", (r) => {
    if (business(r))
      posts.push({
        key: r.headers()["idempotency-key"],
        body: r.postDataJSON(),
      });
  });
  await page.reload();
  await expect(recovery(page)).toBeVisible();
  const converted = await db(page, "get", scope);
  expect(converted.version).toBe(2);
  expect(converted.generation).toBe(legacy.generation);
  expect(converted.attempt.key).toBe(a.key);
  expect(JSON.stringify(converted)).not.toContain(a.body.name);
  expect(posts).toEqual([]);
  await recovery(page)
    .getByRole("button", { name: "Check original save", exact: true })
    .click();
  await expect(recovery(page)).toHaveCount(0);
  const customers = await rows(page, a.body.name);
  expect(customers).toEqual([originalResult.data]);
  expect(posts).toEqual([{ key: a.key, body: a.body }]);
  await record(info, "legacy-conversion.json", {
    name: a.body.name,
    legacy,
    converted,
    posts,
    customers,
  });
});

test("failed legacy sealing preserves plaintext bytes and blocks business application until conversion can finish", async ({
  page,
}, info) => {
  const a = attempt(`Owned legacy outage ${randomUUID().slice(0, 8)}`),
    scope = await owner(page),
    legacy = { version: 1, generation: randomUUID(), attempt: a };
  await db(page, "set", scope, legacy);
  const posts = [];
  page.on("request", (r) => {
    if (business(r)) posts.push(r.url());
  });
  const outage = (r) =>
    r.fulfill({
      status: 503,
      contentType: "application/json",
      body: JSON.stringify({ error: { message: "Owned encryption outage" } }),
    });
  await page.route("**/api/freight/save-journal", outage);
  await page.reload();
  await expect(storage(page)).toContainText("Owned encryption outage");
  expect(await db(page, "get", scope)).toEqual(legacy);
  expect(posts).toEqual([]);
  // Background recovery may also retry a legacy conversion. Keep successful
  // seals pending until the explicit retry has actually received its click.
  const resume = await holdSuccessfulJournalReplies(page, "seal");
  await page.unroute("**/api/freight/save-journal", outage);
  await storage(page)
    .getByRole("button", { name: "Retry recovery storage", exact: true })
    .click();
  await resume();
  await expect(recovery(page)).toBeVisible();
  const converted = await db(page, "get", scope);
  expect(converted.version).toBe(2);
  expect(converted.attempt.key).toBe(a.key);
  expect(JSON.stringify(converted)).not.toContain(a.body.name);
  await recovery(page)
    .getByText("Stop waiting for this save", { exact: true })
    .click();
  await recovery(page)
    .getByLabel("Resolution note")
    .fill("Stop the retained legacy original after secure conversion");
  await recovery(page)
    .getByRole("button", { name: "Resolve original save", exact: true })
    .click();
  await expect(recovery(page)).toHaveCount(0);
  const customers = await rows(page, a.body.name);
  expect(customers).toEqual([]);
  await record(info, "legacy-outage.json", {
    name: a.body.name,
    legacy,
    converted,
    posts,
    customers,
  });
});

test("ciphertext corruption is retained without replay and recovery succeeds after the original bytes are restored", async ({
  page,
}, info) => {
  const a = attempt(`Owned corrupted envelope ${randomUUID().slice(0, 8)}`);
  await uncertain(page, a);
  const scope = await owner(page),
    raw = await db(page, "get", scope),
    broken = structuredClone(raw);
  const c = broken.attempt.sealed.ciphertext;
  broken.attempt.sealed.ciphertext = (c[0] === "A" ? "B" : "A") + c.slice(1);
  await db(page, "set", scope, broken);
  const posts = [];
  page.on("request", (r) => {
    if (business(r)) posts.push(r.url());
  });
  await page.reload();
  await expect(storage(page)).toContainText("could not be verified");
  expect(await db(page, "get", scope)).toEqual(broken);
  expect(posts).toEqual([]);
  // Keep a valid background decode from removing the retry control before
  // this scenario can exercise the explicit recovery action.
  const resume = await holdSuccessfulJournalReplies(page, "open");
  await db(page, "set", scope, raw);
  await storage(page)
    .getByRole("button", { name: "Retry recovery storage", exact: true })
    .click();
  await resume();
  await expect(recovery(page)).toBeVisible();
  await page.unroute("**/api/freight/customers");
  await recovery(page)
    .getByRole("button", { name: "Check original save", exact: true })
    .click();
  await expect(recovery(page)).toHaveCount(0);
  const customers = await rows(page, a.body.name);
  expect(customers).toHaveLength(1);
  await record(info, "ciphertext-corruption.json", {
    name: a.body.name,
    raw,
    broken,
    posts,
    customers,
  });
});

test("invalid and lost seal acknowledgements cannot dispatch a business save or leave a plaintext journal", async ({
  page,
}, info) => {
  const a = attempt(`Owned lost seal ${randomUUID().slice(0, 8)}`),
    d = await form(page, a),
    scope = await owner(page);
  let seals = 0;
  const posts = [];
  page.on("request", (r) => {
    if (business(r)) posts.push(r.url());
  });
  await page.route("**/api/freight/save-journal", async (r) => {
    if (r.request().postDataJSON().action === "seal" && ++seals <= 2) {
      if (seals === 1)
        return r.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({ data: { version: 1 } }),
        });
      await r.fetch();
      return r.abort("failed");
    }
    return r.continue();
  });
  await d.getByRole("button", { name: "Save record", exact: true }).click();
  await expect(storage(page)).toBeVisible();
  expect(posts).toEqual([]);
  expect((await db(page, "get", scope)).attempt).toBe(null);
  await storage(page)
    .getByRole("button", { name: "Retry recovery storage", exact: true })
    .click();
  await expect(storage(page)).toHaveCount(0);
  await d.getByRole("button", { name: "Save record", exact: true }).click();
  await expect(storage(page)).toBeVisible();
  expect(posts).toEqual([]);
  expect((await db(page, "get", scope)).attempt).toBe(null);
  await storage(page)
    .getByRole("button", { name: "Retry recovery storage", exact: true })
    .click();
  await expect(storage(page)).toHaveCount(0);
  await d.getByRole("button", { name: "Save record", exact: true }).click();
  await expect(d).toHaveCount(0);
  const customers = await rows(page, a.body.name);
  expect(customers).toHaveLength(1);
  expect(posts).toHaveLength(1);
  await record(info, "lost-seal-response.json", {
    name: a.body.name,
    seals,
    posts,
    customers,
  });
});

test("the live identity protocol binds encrypted recovery to the verified Clerk organization and session", async ({
  page,
  browser,
}, info) => {
  const origin = clerkOrigin,
    context = await browser.newContext({ baseURL: origin });
  const fixture = async (path, data = {}) => {
    const r = await context.request.post(
      `${fixtureOrigin}/__identity/${path}`,
      { headers: { "x-fixture-key": "freight-fixture-only" }, data },
    );
    expect(r.ok()).toBe(true);
    return r.json();
  };
  try {
    await fixture("control", {
      status: "active",
      role: "org:operations",
      member: true,
      org: "org_staff",
      outage: false,
    });
    const { token } = await fixture("token");
    const headers = {
      Origin: origin,
      Authorization: `Bearer ${token}`,
      "X-Freight-Organization-Id": "org_staff",
    };
    const scope = JSON.stringify([
        "clerk",
        "org_staff",
        "identity",
        "clerk:user_staff",
        "sess_staff",
      ]),
      a = attempt(`PRIVATE Clerk ${randomUUID().slice(0, 8)}`),
      { key, path, body, message, createdAt } = a,
      intent = { key, path, body, message, createdAt };
    const sealedResponse = await context.request.post(
      "/api/freight/save-journal",
      { headers, data: { action: "seal", owner: scope, intent } },
    );
    expect(sealedResponse.ok(), await sealedResponse.text()).toBe(true);
    const sealed = (await sealedResponse.json()).data;
    const open = await context.request.post("/api/freight/save-journal", {
      headers,
      data: { action: "open", owner: scope, sealed },
    });
    expect(open.ok()).toBe(true);
    expect((await open.json()).data).toEqual(intent);
    const denied = [];
    for (const changed of [
      ["clerk", "org_staff", "identity", "clerk:user_staff", "sess_other"],
      ["clerk", "org_other", "identity", "clerk:user_staff", "sess_staff"],
      ["clerk", "org_staff", "identity", "clerk:other_user", "sess_staff"],
    ]) {
      const r = await context.request.post("/api/freight/save-journal", {
        headers,
        data: { action: "open", owner: JSON.stringify(changed), sealed },
      });
      expect(r.status()).toBe(409);
      const result = await r.json();
      expect(JSON.stringify(result)).not.toContain(a.body.name);
      denied.push({ status: r.status(), result });
    }
    const foreignOrigin = await context.request.post(
      "/api/freight/save-journal",
      {
        headers: { ...headers, Origin: "https://unrelated.fixture.test" },
        data: { action: "open", owner: scope, sealed },
      },
    );
    expect(foreignOrigin.status()).toBe(403);
    expect(await foreignOrigin.text()).not.toContain(a.body.name);
    await fixture("control", { status: "revoked" });
    const revoked = await context.request.post("/api/freight/save-journal", {
      headers,
      data: { action: "open", owner: scope, sealed },
    });
    expect(revoked.status()).toBe(401);
    await fixture("control", {
      status: "active",
      org: "org_carrier",
      role: "org:carrier",
      member: true,
    });
    const partnerToken = (await fixture("token")).token;
    const partner = await context.request.post("/api/freight/save-journal", {
      headers: {
        Origin: origin,
        Authorization: `Bearer ${partnerToken}`,
        "X-Freight-Organization-Id": "org_carrier",
      },
      data: { action: "open", owner: scope, sealed },
    });
    expect(partner.status()).toBe(403);
    expect(await partner.text()).not.toContain(a.body.name);
    await record(info, "clerk-seal-boundary.json", {
      scope,
      sealed,
      denied,
      revoked_status: revoked.status(),
      foreign_origin_status: foreignOrigin.status(),
      partner_status: partner.status(),
      scope_note:
        "Actual API JWT/live-session protocol fixture; no live Clerk account or SDK commissioning.",
    });
  } finally {
    await fixture("control", {
      status: "active",
      role: "org:operations",
      member: true,
      org: "org_staff",
      outage: false,
    });
    await context.close();
  }
});

test("sign-out during a pending legacy conversion clears plaintext and fences its late encrypted response", async ({
  page,
}, info) => {
  const a = attempt(`Owned conversion signout ${randomUUID().slice(0, 8)}`),
    scope = await owner(page),
    legacy = { version: 1, generation: randomUUID(), attempt: a };
  await db(page, "set", scope, legacy);
  let held;
  const ready = new Promise((resolve) => {
    held = resolve;
  });
  await page.route("**/api/freight/save-journal", async (route) => {
    const response = await route.fetch();
    held({ route, response });
  });
  await page.reload();
  const sealed = await ready;
  await page.getByRole("button", { name: /Operations team.*Sign out/ }).click();
  await expect(page.getByLabel("Workspace credential")).toBeVisible();
  const cleared = await db(page, "get", scope);
  expect(cleared.version).toBe(2);
  expect(cleared.attempt).toBe(null);
  expect(cleared.generation).not.toBe(legacy.generation);
  await sealed.route.fulfill({ response: sealed.response });
  await page.clock.runFor(50);
  expect(await db(page, "get", scope)).toEqual(cleared);
  await page.unroute("**/api/freight/save-journal");
  await login(page);
  await expect(recovery(page)).toHaveCount(0);
  const customers = await rows(page, a.body.name);
  expect(customers).toEqual([]);
  await record(info, "legacy-signout-fence.json", {
    name: a.body.name,
    legacy,
    cleared,
    customers,
  });
});

test("a changed browser generation during conversion closes the old view even without notification delivery", async ({
  page,
}, info) => {
  const a = attempt(`Owned conversion generation ${randomUUID().slice(0, 8)}`),
    scope = await owner(page),
    legacy = { version: 1, generation: randomUUID(), attempt: a };
  await db(page, "set", scope, legacy);
  let held;
  const ready = new Promise((resolve) => {
    held = resolve;
  });
  await page.route("**/api/freight/save-journal", async (route) => {
    const response = await route.fetch();
    held({ route, response });
  });
  await page.reload();
  const sealed = await ready;
  const replacement = { version: 2, generation: randomUUID(), attempt: null };
  await db(page, "set", scope, replacement);
  await sealed.route.fulfill({ response: sealed.response });
  await expect(page.getByLabel("Workspace credential")).toBeVisible();
  expect(await db(page, "get", scope)).toEqual(replacement);
  expect(await rows(page, a.body.name)).toEqual([]);
  await record(info, "conversion-generation-fence.json", {
    name: a.body.name,
    legacy,
    replacement,
    customers: [],
  });
});
