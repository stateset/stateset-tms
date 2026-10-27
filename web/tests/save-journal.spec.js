const { test, expect } = require("@playwright/test");
const { randomUUID } = require("node:crypto");
const { writeFile } = require("node:fs/promises");
const operator = "freight-test-operator-credential-at-least-32-chars";
const otherTenant = "freight-test-quote-source-credential-at-least-32-chars";
const otherActor = "freight-test-operations-credential-at-least-32-chars";
const recovery = (page) =>
  page.getByRole("region", { name: "Save recovery", exact: true });
const storage = (page) =>
  page.getByRole("region", { name: "Recovery storage", exact: true });
const unique = () => `Owned durable ${randomUUID().slice(0, 8)}`;
async function login(page, credential = operator) {
  await page.goto("/");
  const credentialInput = page.getByLabel("Workspace credential");
  const overview = page.getByRole("heading", { name: "Overview", exact: true });
  // Access resolves after navigation. Wait for a settled signed-in or signed-out
  // view before deciding whether this page needs credentials.
  await expect(credentialInput.or(overview)).toBeVisible();
  if (await credentialInput.isVisible()) {
    await credentialInput.fill(credential);
    await page
      .getByRole("button", { name: "Open workspace", exact: true })
      .click();
  }
  await expect(overview).toBeVisible();
}
async function form(page, name) {
  if (page.viewportSize().width < 761)
    await page
      .getByRole("button", { name: "Toggle navigation", exact: true })
      .click();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Customers", exact: true })
    .click();
  await page.getByRole("button", { name: "Add customer", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Customer name", { exact: true }).fill(name);
  await dialog
    .getByLabel("Contact email or phone")
    .fill("owned-durable@example.test");
  return dialog;
}
async function save(dialog) {
  await dialog
    .getByRole("button", { name: "Save record", exact: true })
    .click();
}
async function rows(page, name) {
  const response = await page.request.get("/api/freight/workspace");
  expect(response.ok()).toBe(true);
  return (await response.json()).customers.filter((c) => c.name === name);
}
async function journal(page, rewrite) {
  return page.evaluate(async (rewrite) => {
    const db = await new Promise((resolve, reject) => {
      const request = indexedDB.open("stateset-freight-save-recovery");
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error);
    });
    try {
      return await new Promise((resolve, reject) => {
        const tx = db.transaction("owners", rewrite ? "readwrite" : "readonly");
        const store = tx.objectStore("owners"),
          request = store.getAll();
        let values;
        request.onsuccess = () => {
          values = request.result;
        };
        if (rewrite) {
          const cursor = store.openCursor();
          cursor.onsuccess = () => {
            const entry = cursor.result;
            if (!entry) return;
            if (entry.value.attempt)
              entry.update({ ...entry.value, version: rewrite.version });
            entry.continue();
          };
        }
        tx.oncomplete = () => resolve(values);
        tx.onabort = () => reject(tx.error);
      });
    } finally {
      db.close();
    }
  }, rewrite);
}
async function fixture(testInfo, name, value) {
  await writeFile(testInfo.outputPath(name), JSON.stringify(value, null, 2));
}
async function changeCookie(page, token) {
  const r = await page.request.post("/api/session", {
    headers: { Origin: new URL(page.url()).origin },
    data: { token },
  });
  expect(r.ok()).toBe(true);
}
test.beforeEach(async ({ page }) => {
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned synthetic API/gateway/browser storage",
  );
  await page.clock.install();
});
for (const committed of [true, false]) {
  test(`reload restores the exact ${committed ? "committed" : "unapplied"} original without automatic submission`, async ({
    page,
  }, testInfo) => {
    if (!committed) await page.setViewportSize({ width: 390, height: 844 });
    await login(page);
    const name = unique(),
      dialog = await form(page, name),
      attempts = [];
    await page.route("**/api/freight/customers", async (route) => {
      attempts.push({
        key: route.request().headers()["idempotency-key"],
        body: route.request().postDataJSON(),
      });
      if (attempts.length === 1) {
        // Observe the actual journal before allowing any HTTP application.
        attempts[0].journalBeforeHttp = await journal(page);
        if (committed) {
          const r = await route.fetch();
          attempts[0].result = await r.json();
        }
        return route.abort("failed");
      }
      await route.continue();
    });
    await save(dialog);
    await expect(recovery(page)).toContainText("Save outcome not confirmed");
    const before = await journal(page);
    await page.reload();
    await expect(recovery(page)).toContainText("Save outcome not confirmed");
    expect(attempts).toHaveLength(1);
    await recovery(page)
      .getByText("Original save details", { exact: true })
      .click();
    await expect(
      recovery(page).getByLabel("Original save values"),
    ).toContainText(name);
    expect(await journal(page)).toEqual(before);
    await recovery(page)
      .getByRole("button", { name: "Check original save", exact: true })
      .click();
    await expect(recovery(page)).toHaveCount(0);
    const customers = await rows(page, name),
      after = await journal(page);
    await fixture(testInfo, `reload-${committed}.json`, {
      name,
      attempts,
      before,
      after,
      customers,
    });
    expect(attempts).toHaveLength(2);
    expect(attempts[1].key).toBe(attempts[0].key);
    expect(attempts[1].body).toEqual(attempts[0].body);
    expect(
      attempts[0].journalBeforeHttp.filter((r) => r.attempt)[0].attempt.key,
    ).toBe(attempts[0].key);
    expect(customers).toHaveLength(1);
    expect(after.every((r) => r.attempt === null)).toBe(true);
  });
}
test("closing the tab during an acknowledged-to-the-server save retains its original before the browser decodes a result", async ({
  page,
  context,
}, testInfo) => {
  await login(page);
  const name = unique(),
    dialog = await form(page, name);
  let original, committed;
  const ready = new Promise((resolve) => {
    committed = resolve;
  });
  await page.route("**/api/freight/customers", async (route) => {
    original = {
      key: route.request().headers()["idempotency-key"],
      body: route.request().postDataJSON(),
    };
    const response = await route.fetch();
    original.result = await response.json();
    committed();
    // Leave the successful response held while the actual browser page closes.
  });
  await save(dialog);
  await ready;
  expect((await journal(page)).find((r) => r.attempt).attempt.key).toBe(
    original.key,
  );
  await page.close();
  const reopened = await context.newPage();
  await reopened.clock.install();
  const posts = [];
  reopened.on("request", (r) => {
    if (r.method() === "POST" && !r.url().endsWith("/api/freight/save-journal"))
      posts.push({
        url: r.url(),
        key: r.headers()["idempotency-key"],
        body: r.postDataJSON(),
      });
  });
  await reopened.goto("/");
  await expect(recovery(reopened)).toContainText("Save outcome not confirmed");
  expect(posts).toEqual([]);
  await recovery(reopened)
    .getByRole("button", { name: "Check original save", exact: true })
    .click();
  await expect(recovery(reopened)).toHaveCount(0);
  const customers = await rows(reopened, name);
  expect(customers).toHaveLength(1);
  expect(posts).toHaveLength(1);
  expect(posts[0].key).toBe(original.key);
  expect(posts[0].body).toEqual(original.body);
  await fixture(testInfo, "reopened-tab.json", {
    name,
    original,
    posts,
    customers,
  });
});
test("a confirmed save survives reload and a failed refresh while recovery remains read only", async ({
  page,
}, testInfo) => {
  await login(page);
  const name = unique(),
    dialog = await form(page, name),
    posts = [];
  page.on("request", (r) => {
    if (
      r.method() === "POST" &&
      r.url().includes("/api/freight/") &&
      !r.url().endsWith("/api/freight/save-journal")
    )
      posts.push(r.url());
  });
  await page.route("**/api/freight/workspace?history=deferred", (route) =>
    route.fulfill({
      status: 503,
      contentType: "application/json",
      body: JSON.stringify({ error: { message: "Owned refresh outage" } }),
    }),
  );
  await save(dialog);
  await expect(recovery(page)).toContainText("Save completed");
  await page.reload();
  await expect(recovery(page)).toContainText("Save completed");
  expect(posts).toHaveLength(1);
  await recovery(page)
    .getByRole("button", { name: "Refresh workspace", exact: true })
    .click();
  await expect(recovery(page)).toContainText("Owned refresh outage");
  await page.unroute("**/api/freight/workspace?history=deferred");
  await recovery(page)
    .getByRole("button", { name: "Refresh workspace", exact: true })
    .click();
  await expect(recovery(page)).toHaveCount(0);
  const customers = await rows(page, name);
  expect(customers).toHaveLength(1);
  expect(posts).toHaveLength(1);
  await fixture(testInfo, "confirmed-reload.json", { name, posts, customers });
});
test("a lost stop decision survives reload and the original check recognizes its durable fence", async ({
  page,
}, testInfo) => {
  await login(page);
  const name = unique(),
    dialog = await form(page, name);
  let original;
  await page.route("**/api/freight/customers", async (route) => {
    original = {
      key: route.request().headers()["idempotency-key"],
      body: route.request().postDataJSON(),
    };
    await route.abort("failed");
  });
  await save(dialog);
  await expect(recovery(page)).toBeVisible();
  let decision;
  await page.route("**/api/freight/save-recovery/resolve", async (route) => {
    const r = await route.fetch();
    decision = await r.json();
    await route.abort("failed");
  });
  await recovery(page)
    .getByText("Stop waiting for this save", { exact: true })
    .click();
  await recovery(page)
    .getByLabel("Resolution note")
    .fill("Owned stop across reload");
  await recovery(page)
    .getByRole("button", { name: "Resolve original save", exact: true })
    .click();
  await expect(recovery(page)).toContainText("Failed to fetch");
  await page.reload();
  await expect(recovery(page)).toContainText("Save outcome not confirmed");
  await page.unroute("**/api/freight/customers");
  await recovery(page)
    .getByRole("button", { name: "Check original save", exact: true })
    .click();
  await expect(recovery(page)).toHaveCount(0);
  const customers = await rows(page, name);
  expect(customers).toEqual([]);
  expect(decision.data.outcome).toBe("stopped");
  await fixture(testInfo, "stopped-reload.json", {
    name,
    original,
    decision,
    customers,
  });
});
test("two stale tabs atomically retain one original even when cross-tab notifications are unavailable", async ({
  page,
  context,
}, testInfo) => {
  await context.addInitScript(() => {
    globalThis.BroadcastChannel = undefined;
  });
  await login(page);
  const second = await context.newPage();
  await second.clock.install();
  await second.goto("/");
  await expect(
    second.getByRole("heading", { name: "Overview", exact: true }),
  ).toBeVisible();
  const names = [unique(), unique()],
    forms = [await form(page, names[0]), await form(second, names[1])],
    posts = [];
  for (const tab of [page, second])
    await tab.route("**/api/freight/customers", async (route) => {
      posts.push({
        key: route.request().headers()["idempotency-key"],
        body: route.request().postDataJSON(),
      });
      await route.abort("failed");
    });
  await Promise.all(forms.map(save));
  for (const tab of [page, second])
    await expect(recovery(tab)).toContainText("Save outcome not confirmed");
  expect(posts).toHaveLength(1);
  const retained = await journal(page);
  expect(retained.filter((r) => r.attempt)).toHaveLength(1);
  const access = await (await page.request.get("/api/freight/access")).json();
  const owner = JSON.stringify([
    "credential",
    "",
    access.tenant_id,
    access.access.subject,
    "",
  ]);
  const opened = await page.request.post("/api/freight/save-journal", {
    headers: { Origin: new URL(page.url()).origin },
    data: {
      action: "open",
      owner,
      sealed: retained.find((r) => r.attempt).attempt.sealed,
    },
  });
  expect(opened.ok()).toBe(true);
  expect((await opened.json()).data.body).toEqual(posts[0].body);
  expect(JSON.stringify(retained)).not.toContain(posts[0].body.name);
  for (const name of names) expect(await rows(page, name)).toEqual([]);
  await fixture(testInfo, "concurrent-tabs.json", { names, posts, retained });
});
test("restoration is isolated from a different tenant and a different operator in the same tenant", async ({
  page,
}, testInfo) => {
  await login(page);
  const name = unique(),
    dialog = await form(page, name);
  await page.route("**/api/freight/customers", (route) =>
    route.abort("failed"),
  );
  await save(dialog);
  await expect(recovery(page)).toBeVisible();
  const retained = await journal(page);
  for (const token of [otherTenant, otherActor]) {
    await changeCookie(page, token);
    await page.reload();
    await expect(
      page.getByRole("heading", { name: "Overview", exact: true }),
    ).toBeVisible();
    await expect(recovery(page)).toHaveCount(0);
    await expect(page.getByText(name, { exact: true })).toHaveCount(0);
  }
  await changeCookie(page, operator);
  await page.reload();
  await expect(recovery(page)).toContainText("Save outcome not confirmed");
  const restored = await journal(page);
  expect(restored.find((r) => r.attempt).attempt).toEqual(
    retained.find((r) => r.attempt).attempt,
  );
  await fixture(testInfo, "owner-isolation.json", { name, retained, restored });
});
test("a changed credential cookie cannot replay a restored body into another tenant", async ({
  page,
}, testInfo) => {
  await login(page);
  const name = unique(),
    dialog = await form(page, name);
  await page.route("**/api/freight/customers", (route) =>
    route.abort("failed"),
  );
  await save(dialog);
  await expect(recovery(page)).toBeVisible();
  await page.unroute("**/api/freight/customers");
  await changeCookie(page, otherTenant);
  const response = page.waitForResponse(
    (r) => new URL(r.url()).pathname === "/api/freight/save-journal",
  );
  await recovery(page)
    .getByRole("button", { name: "Check original save", exact: true })
    .click();
  const failure = await response;
  expect(failure.status()).toBe(409);
  const result = await failure.json();
  expect(result.error.code).toBe("workspace_session_changed");
  await expect(page.getByLabel("Workspace credential")).toBeVisible();
  expect(await rows(page, name)).toEqual([]);
  await changeCookie(page, operator);
  await page.reload();
  await expect(recovery(page)).toContainText("Save outcome not confirmed");
  expect(await rows(page, name)).toEqual([]);
  await fixture(testInfo, "gateway-owner-fence.json", {
    name,
    status: failure.status(),
    result,
    retained: await journal(page),
  });
});
test("an unreadable journal pauses new writes and preserves the original until recovery storage is repaired", async ({
  page,
}, testInfo) => {
  await login(page);
  const name = unique(),
    dialog = await form(page, name);
  let posts = 0;
  await page.route("**/api/freight/customers", (route) => {
    posts++;
    return route.abort("failed");
  });
  await save(dialog);
  await expect(recovery(page)).toBeVisible();
  const before = await journal(page);
  await journal(page, { version: 999 });
  await page.reload();
  await expect(storage(page)).toContainText("cannot be read safely");
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Customers", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "Add customer", exact: true }),
  ).toBeDisabled();
  expect(posts).toBe(1);
  const unreadable = await journal(page);
  expect(unreadable.find((r) => r.attempt).version).toBe(999);
  await journal(page, { version: 2 });
  await storage(page)
    .getByRole("button", { name: "Retry recovery storage", exact: true })
    .click();
  await expect(storage(page)).toHaveCount(0);
  await expect(recovery(page)).toContainText("Save outcome not confirmed");
  expect(await journal(page)).toEqual(before);
  await fixture(testInfo, "unreadable-storage.json", {
    name,
    before,
    unreadable,
    posts,
  });
});
test("a failed journal commit prevents HTTP submission and an explicit retry restores writes", async ({
  page,
}, testInfo) => {
  await login(page);
  const name = unique(),
    dialog = await form(page, name);
  let posts = 0;
  page.on("request", (r) => {
    if (r.method() === "POST" && r.url().includes("/api/freight/customers"))
      posts++;
  });
  await page.evaluate(() => {
    globalThis.ownedOriginalPut = IDBObjectStore.prototype.put;
    IDBObjectStore.prototype.put = function (value, ...args) {
      if (value.attempt)
        throw new DOMException(
          "Owned storage quota refusal",
          "QuotaExceededError",
        );
      return globalThis.ownedOriginalPut.call(this, value, ...args);
    };
  });
  await save(dialog);
  await expect(storage(page)).toContainText("paused");
  expect(posts).toBe(0);
  expect((await journal(page)).every((r) => r.attempt === null)).toBe(true);
  await page.evaluate(() => {
    IDBObjectStore.prototype.put = globalThis.ownedOriginalPut;
  });
  await storage(page)
    .getByRole("button", { name: "Retry recovery storage", exact: true })
    .click();
  await expect(storage(page)).toHaveCount(0);
  await save(dialog);
  await expect(dialog).toHaveCount(0);
  const customers = await rows(page, name);
  expect(posts).toBe(1);
  expect(customers).toHaveLength(1);
  await fixture(testInfo, "storage-before-send.json", {
    name,
    posts,
    customers,
  });
});
test("sign-out clears retained payloads and closes another tab before a late acknowledgement", async ({
  page,
  context,
}, testInfo) => {
  await login(page, otherTenant);
  const second = await context.newPage();
  await second.clock.install();
  await second.goto("/");
  await expect(
    second.getByRole("heading", { name: "Overview", exact: true }),
  ).toBeVisible();
  const name = unique(),
    dialog = await form(page, name);
  let held;
  const ready = new Promise((resolve) => {
    held = resolve;
  });
  await page.route("**/api/freight/customers", async (route) => {
    const response = await route.fetch();
    held({ route, response });
  });
  await save(dialog);
  const original = await ready;
  await second
    .getByRole("button", { name: /Operations team.*Sign out/ })
    .click();
  await expect(page.getByLabel("Workspace credential")).toBeVisible();
  await expect(second.getByLabel("Workspace credential")).toBeVisible();
  const cleared = await journal(page);
  expect(cleared.every((r) => r.attempt === null)).toBe(true);
  await original.route.fulfill({ response: original.response });
  await page.clock.runFor(100);
  await expect(recovery(page)).toHaveCount(0);
  await login(page, otherTenant);
  await expect(recovery(page)).toHaveCount(0);
  const customers = await rows(page, name);
  expect(customers).toHaveLength(1);
  expect((await journal(page)).every((r) => r.attempt === null)).toBe(true);
  await fixture(testInfo, "signout-fence.json", { name, cleared, customers });
});

test("a stale recovery check reveals a newer original without submitting that different request", async ({
  page,
  context,
}, testInfo) => {
  await context.addInitScript(() => {
    globalThis.BroadcastChannel = undefined;
  });
  await login(page);
  const names = [unique(), unique()],
    first = await form(page, names[0]);
  const posts = [];
  await page.route("**/api/freight/customers", async (route) => {
    posts.push({
      key: route.request().headers()["idempotency-key"],
      body: route.request().postDataJSON(),
    });
    await route.abort("failed");
  });
  await save(first);
  await expect(recovery(page)).toBeVisible();
  const second = await context.newPage();
  await second.clock.install();
  await second.goto("/");
  await expect(recovery(second)).toBeVisible();
  await recovery(page)
    .getByText("Stop waiting for this save", { exact: true })
    .click();
  await recovery(page)
    .getByLabel("Resolution note")
    .fill("Owned stop before a deliberately new request");
  // Verify the definitive stop before checking its subsequent read refresh.
  // Neither operation shares the assertion window for the other operation.
  const stopped = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" &&
      new URL(response.url()).pathname === "/api/freight/save-recovery/resolve",
  );
  await recovery(page)
    .getByRole("button", { name: "Resolve original save", exact: true })
    .click();
  const stopResponse = await stopped;
  expect(stopResponse.ok()).toBe(true);
  expect((await stopResponse.json()).data).toMatchObject({
    outcome: "stopped",
    request_key: posts[0].key,
  });
  await expect(recovery(page)).toHaveCount(0);
  await save(await form(page, names[1]));
  await expect(recovery(page)).toBeVisible();
  expect(posts).toHaveLength(2);
  expect(posts[0].key).not.toBe(posts[1].key);
  const stalePosts = [];
  second.on("request", (r) => {
    if (
      r.method() === "POST" &&
      r.url().includes("/api/freight/") &&
      !r.url().endsWith("/api/freight/save-journal")
    )
      stalePosts.push(r.url());
  });
  await recovery(second)
    .getByRole("button", { name: "Check original save", exact: true })
    .click();
  await expect(
    recovery(second).getByRole("button", {
      name: "Check original save",
      exact: true,
    }),
  ).toBeEnabled();
  await recovery(second)
    .getByText("Original save details", { exact: true })
    .click();
  await expect(
    recovery(second).getByLabel("Original save values"),
  ).toContainText(names[1]);
  expect(stalePosts).toEqual([]);
  for (const name of names) expect(await rows(page, name)).toEqual([]);
  await fixture(testInfo, "stale-check.json", {
    names,
    posts,
    stalePosts,
    retained: await journal(page),
  });
});

test("an initial rejection cannot erase an original while another tab is checking that same key", async ({
  page,
  context,
}, testInfo) => {
  await login(page);
  const name = unique(),
    dialog = await form(page, name);
  let firstHeld;
  const firstReady = new Promise((resolve) => {
    firstHeld = resolve;
  });
  await page.route("**/api/freight/customers", (route) => firstHeld(route));
  await save(dialog);
  const first = await firstReady;
  const second = await context.newPage();
  await second.clock.install();
  await second.goto("/");
  await expect(recovery(second)).toBeVisible();
  let secondHeld;
  const secondReady = new Promise((resolve) => {
    secondHeld = resolve;
  });
  await second.route("**/api/freight/customers", async (route) => {
    const response = await route.fetch();
    secondHeld({ route, response });
  });
  await recovery(second)
    .getByRole("button", { name: "Check original save", exact: true })
    .click();
  const checked = await secondReady;
  expect(checked.response.ok()).toBe(true);
  await first.fulfill({
    status: 422,
    contentType: "application/json",
    body: JSON.stringify({
      error: {
        message:
          "Owned transient original rejection while the same-key check commits",
      },
    }),
  });
  await expect(recovery(page)).toContainText("Save outcome not confirmed");
  const pending = await journal(page);
  expect(pending.find((r) => r.attempt).attempt.checks).toBe(1);
  const customers = await rows(page, name);
  expect(customers).toHaveLength(1);
  await checked.route.fulfill({ response: checked.response });
  await expect(recovery(second)).toHaveCount(0);
  await expect(recovery(page)).toHaveCount(0);
  await fixture(testInfo, "overlapping-check.json", {
    name,
    pending,
    customers,
    after: await journal(page),
  });
});

test("a held browser storage transaction expires before any business request and recovers after release", async ({
  page,
}, testInfo) => {
  await login(page);
  const name = unique(),
    dialog = await form(page, name);
  let posts = 0;
  page.on("request", (r) => {
    if (r.method() === "POST" && r.url().includes("/api/freight/customers"))
      posts++;
  });
  await page.evaluate(async () => {
    const db = await new Promise((resolve) => {
      const r = indexedDB.open("stateset-freight-save-recovery");
      r.onsuccess = () => resolve(r.result);
    });
    const tx = db.transaction("owners", "readwrite"),
      store = tx.objectStore("owners");
    let active = true;
    const pump = () => {
      const r = store.get("owned-transaction-hold");
      r.onsuccess = () => {
        if (active) pump();
      };
    };
    pump();
    globalThis.releaseOwnedJournalHold = () => {
      active = false;
      db.close();
      IDBDatabase.prototype.transaction = globalThis.ownedOriginalTransaction;
    };
    globalThis.ownedOriginalTransaction = IDBDatabase.prototype.transaction;
    IDBDatabase.prototype.transaction = function (...args) {
      globalThis.ownedJournalTransactionQueued = true;
      return globalThis.ownedOriginalTransaction.apply(this, args);
    };
  });
  await save(dialog);
  await expect
    .poll(() => page.evaluate(() => globalThis.ownedJournalTransactionQueued))
    .toBe(true);
  await page.clock.fastForward(5001);
  await expect(storage(page)).toContainText("paused");
  expect(posts).toBe(0);
  await page.evaluate(() => globalThis.releaseOwnedJournalHold());
  await storage(page)
    .getByRole("button", { name: "Retry recovery storage", exact: true })
    .click();
  await expect(storage(page)).toHaveCount(0);
  await save(dialog);
  await expect(dialog).toHaveCount(0);
  const customers = await rows(page, name);
  expect(posts).toBe(1);
  expect(customers).toHaveLength(1);
  await fixture(testInfo, "storage-deadline.json", { name, posts, customers });
});

test("native browser transactions clear only the ended Clerk session and fence delayed claims and acknowledgements", async ({
  page,
}, testInfo) => {
  await login(page);
  let source = await require("node:fs/promises").readFile(
    require("node:path").join(__dirname, "../lib/save-journal.mjs"),
    "utf8",
  );
  const model = await require("node:fs/promises").readFile(
    require("node:path").join(__dirname, "../lib/save-intent.mjs"),
    "utf8",
  );
  source = source.replace(
    "./save-intent.mjs",
    `data:text/javascript;base64,${Buffer.from(model).toString("base64")}`,
  );
  const moduleUrl = `data:text/javascript;base64,${Buffer.from(source).toString("base64")}`;
  const observations = await page.evaluate(async (moduleUrl) => {
    const m = await import(moduleUrl),
      access = {
        tenant_id: "owned-native-store",
        access: { subject: "owned-native-subject" },
      };
    const owners = [
      m.journalOwner(access, "clerk", "org_a", "owned-ended-session"),
      m.journalOwner(access, "clerk", "org_b", "owned-ended-session"),
      m.journalOwner(access, "clerk", "org_a", "owned-current-session"),
      m.journalOwner(access, "credential"),
    ];
    const values = new Map();
    const codec = async (action, owner, value) => {
      if (action === "seal") {
        const token = crypto.randomUUID();
        values.set(owner + token.replaceAll("-", ""), structuredClone(value));
        return {
          version: 1,
          key_id: "owned-native-test",
          iv: "A".repeat(16),
          tag: "A".repeat(22),
          ciphertext: token.replaceAll("-", ""),
        };
      }
      return structuredClone(values.get(owner + value.ciphertext));
    };
    const store = (owner, operation, options = {}) =>
      m.saveJournal(owner, operation, { ...options, codec });
    const records = [];
    for (const owner of owners) {
      const initial = await store(owner, "read"),
        body = { name: "Owned native store original" },
        path = "customers",
        attempt = {
          key: crypto.randomUUID(),
          body,
          path,
          fingerprint: JSON.stringify([path, body]),
          kind: "unknown",
          checks: 0,
          createdAt: new Date().toISOString(),
          message: "Customer added",
        };
      records.push(
        await store(owner, "claim", {
          generation: initial.generation,
          attempt,
        }),
      );
    }
    const originalUpdate = IDBCursor.prototype.update;
    let cleanupRefused = false;
    try {
      IDBCursor.prototype.update = function () {
        throw new DOMException(
          "Owned session cleanup refusal",
          "QuotaExceededError",
        );
      };
      try {
        await m.forgetClerkJournalSession("owned-ended-session");
      } catch {
        cleanupRefused = true;
      }
    } finally {
      IDBCursor.prototype.update = originalUpdate;
    }
    const refused = [];
    for (const owner of owners) refused.push(await store(owner, "read"));
    await m.forgetClerkJournalSession("owned-ended-session");
    const after = [];
    for (const owner of owners) after.push(await store(owner, "read"));
    const fences = [];
    for (const operation of ["claim", "confirmed"]) {
      try {
        await store(owners[0], operation, {
          generation: records[0].generation,
          attempt: records[0].attempt,
          key: records[0].attempt.key,
        });
        fences.push(false);
      } catch {
        fences.push(true);
      }
    }
    return { before: records, after, fences, cleanupRefused, refused };
  }, moduleUrl);
  expect(observations.cleanupRefused).toBe(true);
  expect(observations.refused).toEqual(
    observations.before.map(({ claimed, ...record }) => record),
  );
  expect(observations.after.slice(0, 2).every((r) => r.attempt === null)).toBe(
    true,
  );
  expect(observations.after.slice(2).every((r) => !!r.attempt)).toBe(true);
  expect(observations.fences).toEqual([true, true]);
  expect(observations.after[0].generation).not.toBe(
    observations.before[0].generation,
  );
  await fixture(testInfo, "clerk-session-storage.json", observations);
});

test("storage cleanup refusal still deletes the session cookie and keeps local cleanup explicitly retryable", async ({
  page,
}, testInfo) => {
  await login(page, otherTenant);
  const name = unique(),
    dialog = await form(page, name);
  await page.route("**/api/freight/customers", (route) =>
    route.abort("failed"),
  );
  await save(dialog);
  await expect(recovery(page)).toBeVisible();
  await page.keyboard.press("Escape");
  const before = await journal(page);
  let deletes = 0;
  await page.route("**/api/session", (route) => {
    if (route.request().method() === "DELETE") deletes++;
    return route.continue();
  });
  await page.evaluate(() => {
    globalThis.ownedOriginalOpen = IDBFactory.prototype.open;
    IDBFactory.prototype.open = function () {
      throw new DOMException("Owned cleanup refusal", "SecurityError");
    };
  });
  await page.getByRole("button", { name: /Operations team.*Sign out/ }).click();
  await expect(page.locator(".form-error[role=alert]")).toContainText(
    "You are signed out. Browser save recovery could not be cleared.",
  );
  expect(deletes).toBe(1);
  const denied = await page.request.get("/api/freight/access");
  expect(denied.status()).toBe(401);
  await expect(
    page.getByRole("button", { name: "Open workspace", exact: true }),
  ).toBeDisabled();
  await page.evaluate(() => {
    IDBFactory.prototype.open = globalThis.ownedOriginalOpen;
  });
  expect(await journal(page)).toEqual(before);
  await page
    .getByRole("button", { name: "Retry sign out", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "Open workspace", exact: true }),
  ).toBeEnabled();
  const after = await journal(page);
  expect(after.every((r) => r.attempt === null)).toBe(true);
  expect(deletes).toBe(2);
  await login(page, otherTenant);
  await expect(recovery(page)).toHaveCount(0);
  const customers = await rows(page, name);
  expect(customers).toEqual([]);
  await fixture(testInfo, "signout-storage-failure.json", {
    name,
    before,
    after,
    deletes,
    access_status_after_refused_cleanup: denied.status(),
    customers,
  });
});
