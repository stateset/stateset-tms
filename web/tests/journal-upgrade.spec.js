const { staffOrigin, apiOrigin } = require("./service-origins.cjs")();
const { test, expect } = require("@playwright/test");
const { randomUUID, createHash } = require("node:crypto");
const { readFile, writeFile } = require("node:fs/promises");
const path = require("node:path");
const DATABASE = "stateset-freight-save-recovery";
const credential = "freight-test-operator-credential-at-least-32-chars";
const other = "freight-test-quote-source-credential-at-least-32-chars";
const third = "freight-test-operations-credential-at-least-32-chars";
const recovery = (p) =>
  p.getByRole("region", { name: "Save recovery", exact: true });
const storage = (p) =>
  p.getByRole("region", { name: "Recovery storage", exact: true });
const business = (r) =>
  r.method() === "POST" &&
  r.url().includes("/api/freight/") &&
  !r.url().endsWith("/api/freight/save-journal");
const output = (info, name, value) =>
  writeFile(info.outputPath(name), JSON.stringify(value, null, 2));
let modules, manifest;
test.beforeAll(async () => {
  const base = path.join(__dirname, "fixtures/journal");
  manifest = JSON.parse(
    await readFile(path.join(base, "manifest.json"), "utf8"),
  );
  const files = {};
  for (const [name, entry] of Object.entries(manifest.files)) {
    const bytes = await readFile(path.join(base, name));
    expect(createHash("sha256").update(bytes).digest("hex")).toBe(entry.sha256);
    const evidence = await readFile(
      path.join(__dirname, "../..", entry.qualification),
    );
    expect(createHash("sha256").update(evidence).digest("hex")).toBe(
      entry.qualification_sha256,
    );
    expect(
      JSON.parse(evidence).runnable_sources_sha256[entry.qualified_source],
    ).toBe(entry.sha256);
    files[name] = bytes.toString("utf8");
  }
  const url = (s) =>
    `data:text/javascript;base64,${Buffer.from(s).toString("base64")}`;
  modules = {
    plain: url(files["plaintext-v1.mjs"]),
    encrypted: url(
      files["encrypted-v2-db1.mjs"].replace(
        "./save-intent.mjs",
        url(files["save-intent.mjs"]),
      ),
    ),
  };
});
async function catalog(page) {
  await page.goto("/catalog");
  await expect(
    page.getByRole("region", { name: "Shipment results" }),
  ).toBeVisible();
}
async function signIn(page, token = credential) {
  const r = await page.request.post("/api/session", {
    headers: { Origin: staffOrigin },
    data: { token },
  });
  expect(r.ok()).toBe(true);
}
async function owner(page, token) {
  const response = token
    ? await page.request.get(`${apiOrigin}/v1/access`, {
        headers: { Authorization: `Bearer ${token}` },
      })
    : await page.request.get("/api/freight/access");
  expect(response.ok()).toBe(true);
  const a = await response.json();
  return JSON.stringify(["credential", "", a.tenant_id, a.access.subject, ""]);
}
function attempt() {
  const name = "Owned database fence " + randomUUID().slice(0, 8),
    body = {
      name,
      contact: "owned-fence@example.test",
      billing_address: "Owned retained address",
    },
    requestPath = "customers";
  return {
    key: randomUUID(),
    path: requestPath,
    body,
    fingerprint: JSON.stringify([requestPath, body]),
    kind: "unknown",
    checks: 0,
    message: "Customer added",
    createdAt: new Date().toISOString(),
  };
}
async function old(page, which, scope, operation, options = {}) {
  return page.evaluate(
    async ({ url, scope, operation, options }) =>
      (await import(url)).saveJournal(scope, operation, options),
    { url: modules[which], scope, operation, options },
  );
}
async function inspect(page) {
  return page.evaluate(async () => {
    const db = await new Promise((resolve, reject) => {
      const r = indexedDB.open("stateset-freight-save-recovery");
      r.onsuccess = () => resolve(r.result);
      r.onerror = () => reject(r.error);
    });
    try {
      const names = [...db.objectStoreNames];
      if (!names.length) return { version: db.version, stores: [] };
      return await new Promise((resolve, reject) => {
        const tx = db.transaction(names, "readonly"),
          stores = names.map((name) => {
            const s = tx.objectStore(name),
              v = { name, keyPath: s.keyPath, autoIncrement: s.autoIncrement };
            s.getAllKeys().onsuccess = (e) => {
              v.keys = e.target.result;
            };
            s.getAll().onsuccess = (e) => {
              v.values = e.target.result;
            };
            return v;
          });
        tx.oncomplete = () => resolve({ version: db.version, stores });
        tx.onabort = () => reject(tx.error);
      });
    } finally {
      db.close();
    }
  });
}
function row(state, key, store = "owners") {
  const s = state.stores.find((v) => v.name === store);
  return s.values[s.keys.indexOf(key)];
}
async function put(page, key, value) {
  await page.evaluate(
    async ({ key, value }) => {
      const db = await new Promise((resolve) => {
        const r = indexedDB.open("stateset-freight-save-recovery");
        r.onsuccess = () => resolve(r.result);
      });
      try {
        await new Promise((resolve, reject) => {
          const t = db.transaction("owners", "readwrite");
          t.objectStore("owners").put(value, key);
          t.oncomplete = resolve;
          t.onabort = () => reject(t.error);
        });
      } finally {
        db.close();
      }
    },
    { key, value },
  );
}
async function rows(page, name) {
  const r = await page.request.get("/api/freight/workspace");
  expect(r.ok()).toBe(true);
  return (await r.json()).customers.filter((c) => c.name === name);
}
async function nativeVersionError(page) {
  return page.evaluate(
    () =>
      new Promise((resolve) => {
        const r = indexedDB.open("stateset-freight-save-recovery", 1);
        r.onerror = () => resolve(r.error.name);
        r.onsuccess = () => {
          r.result.close();
          resolve("unexpected success");
        };
      }),
  );
}
async function stop(page) {
  await recovery(page)
    .getByText("Stop waiting for this save", { exact: true })
    .click();
  await recovery(page)
    .getByLabel("Resolution note")
    .fill("Stop the original after preserving the database upgrade boundary");
  await recovery(page)
    .getByRole("button", { name: "Resolve original save", exact: true })
    .click();
  await expect(recovery(page)).toHaveCount(0);
}
test.beforeEach(async ({ page }) => {
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned prior clients, API and native browser databases",
  );
  await page.clock.install();
  await signIn(page);
  await catalog(page);
});

test("the actual old clients reproduce plaintext reset before upgrade and cannot write after the native fence commits", async ({
  page,
}, info) => {
  const scope = await owner(page),
    first = attempt(),
    original = attempt(),
    posts = [];
  page.on("request", (r) => {
    if (business(r))
      posts.push({
        key: r.headers()["idempotency-key"],
        body: r.postDataJSON(),
      });
  });
  const initial = await old(page, "encrypted", scope, "read");
  await old(page, "encrypted", scope, "claim", {
    generation: initial.generation,
    attempt: first,
  });
  const encrypted = await inspect(page);
  expect(encrypted.version).toBe(1);
  expect(row(encrypted, scope).version).toBe(2);
  const reset = await old(page, "plain", scope, "forget", {
    generation: initial.generation,
  });
  await old(page, "plain", scope, "claim", {
    generation: reset.generation,
    attempt: original,
  });
  const preceding = await inspect(page);
  expect(row(preceding, scope).version).toBe(1);
  expect(JSON.stringify(preceding)).toContain(original.body.name);
  await page.goto("/");
  await expect(recovery(page)).toBeVisible();
  const upgraded = await inspect(page);
  expect(upgraded.version).toBe(2);
  expect(row(upgraded, scope).generation).toBe(reset.generation);
  expect(row(upgraded, scope).attempt.key).toBe(original.key);
  expect(JSON.stringify(upgraded)).not.toContain(original.body.name);
  expect(posts).toEqual([]);
  const refused = [];
  for (const which of ["plain", "encrypted"]) {
    for (const operation of [
      "read",
      "forget",
      "claim",
      "check",
      "confirmed",
      "stopped",
      "remove",
    ]) {
      try {
        await old(page, which, scope, operation, {
          generation: reset.generation,
          key: original.key,
          attempt: attempt(),
        });
        refused.push({ which, operation, rejected: false });
      } catch (error) {
        refused.push({
          which,
          operation,
          rejected: true,
          message: error.message.split("\n")[0],
        });
      }
    }
  }
  expect(refused.every((v) => v.rejected)).toBe(true);
  expect(await nativeVersionError(page)).toBe("VersionError");
  expect(await inspect(page)).toEqual(upgraded);
  expect(posts).toEqual([]);
  await recovery(page)
    .getByRole("button", { name: "Check original save", exact: true })
    .click();
  await expect(recovery(page)).toHaveCount(0);
  const customers = await rows(page, original.body.name);
  expect(customers).toHaveLength(1);
  expect(posts).toEqual([{ key: original.key, body: original.body }]);
  expect(await rows(page, first.body.name)).toEqual([]);
  await output(info, "legacy-writer-fence.json", {
    name: original.body.name,
    first,
    original,
    encrypted,
    preceding,
    upgraded,
    refused,
    native_error: "VersionError",
    posts,
    customers,
    qualified_prior_modules: manifest,
  });
});

test("the structural upgrade retains another owner's original and unknown rows until that owner authenticates", async ({
  page,
}, info) => {
  const a = attempt(),
    b = attempt(),
    c = attempt(),
    scope = await owner(page),
    otherScope = await owner(page, other),
    initial = await old(page, "plain", scope, "read");
  await old(page, "plain", scope, "claim", {
    generation: initial.generation,
    attempt: a,
  });
  const otherRecord = { version: 1, generation: randomUUID(), attempt: b },
    unknown = {
      version: 999,
      generation: randomUUID(),
      extension: { retained: "Owned unknown recovery data" },
    };
  await put(page, otherScope, otherRecord);
  await put(page, "owned-unrecognized-scope", unknown);
  await signIn(page, third);
  const thirdScope = await owner(page),
    encryptedInitial = await old(page, "encrypted", thirdScope, "read");
  await old(page, "encrypted", thirdScope, "claim", {
    generation: encryptedInitial.generation,
    attempt: c,
  });
  await signIn(page);
  const before = await inspect(page);
  const retainedCiphertext = row(before, thirdScope);
  expect(retainedCiphertext.version).toBe(2);
  const posts = [];
  page.on("request", (r) => {
    if (business(r)) posts.push(r.url());
  });
  await page.goto("/");
  await expect(recovery(page)).toBeVisible();
  const ownConverted = await inspect(page);
  expect(ownConverted.version).toBe(2);
  expect(row(ownConverted, otherScope)).toEqual(otherRecord);
  expect(row(ownConverted, thirdScope)).toEqual(retainedCiphertext);
  expect(row(ownConverted, "owned-unrecognized-scope")).toEqual(unknown);
  expect(row(ownConverted, scope).attempt.key).toBe(a.key);
  await signIn(page, other);
  await page.reload();
  await expect(recovery(page)).toBeVisible();
  const otherConverted = await inspect(page);
  expect(row(otherConverted, otherScope).version).toBe(2);
  expect(row(otherConverted, otherScope).generation).toBe(
    otherRecord.generation,
  );
  expect(row(otherConverted, otherScope).attempt.key).toBe(b.key);
  expect(row(otherConverted, scope)).toEqual(row(ownConverted, scope));
  expect(row(otherConverted, thirdScope)).toEqual(retainedCiphertext);
  expect(row(otherConverted, "owned-unrecognized-scope")).toEqual(unknown);
  expect(posts).toEqual([]);
  expect(await rows(page, b.body.name)).toEqual([]);
  await output(info, "multi-owner-upgrade.json", {
    names: [a.body.name, b.body.name, c.body.name],
    scope,
    otherScope,
    thirdScope,
    before,
    ownConverted,
    otherConverted,
    posts,
    customers: [],
  });
});

test("a held legacy connection pauses writes and an abandoned blocked open cannot upgrade later", async ({
  page,
  context,
}, info) => {
  const a = attempt(),
    scope = await owner(page),
    initial = await old(page, "plain", scope, "read");
  await old(page, "plain", scope, "claim", {
    generation: initial.generation,
    attempt: a,
  });
  const before = await inspect(page);
  const blocker = await context.newPage();
  await blocker.clock.install();
  await catalog(blocker);
  await blocker.evaluate(async () => {
    globalThis.ownedLegacyConnection = await new Promise((resolve) => {
      const r = indexedDB.open("stateset-freight-save-recovery", 1);
      r.onsuccess = () => resolve(r.result);
    });
    globalThis.ownedLegacyConnection.onversionchange = () => {
      globalThis.ownedVersionChange = true;
    };
  });
  await page.addInitScript(() => {
    const open = IDBFactory.prototype.open;
    IDBFactory.prototype.open = function (...args) {
      const r = open.apply(this, args);
      if (args[0] === "stateset-freight-save-recovery" && args[1] === 2) {
        r.addEventListener("blocked", () => {
          globalThis.ownedUpgradeBlocked = true;
        });
        r.addEventListener("error", () => {
          globalThis.ownedAbandonedUpgradeError = r.error.name;
        });
      }
      return r;
    };
  });
  const posts = [];
  page.on("request", (r) => {
    if (business(r)) posts.push(r.url());
  });
  await page.goto("/");
  await expect
    .poll(() => blocker.evaluate(() => !!globalThis.ownedVersionChange))
    .toBe(true);
  await expect
    .poll(() => page.evaluate(() => !!globalThis.ownedUpgradeBlocked))
    .toBe(true);
  await page.clock.fastForward(5001);
  await expect(storage(page)).toContainText(
    "Another tab is using older save recovery",
  );
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Customers", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "Add customer", exact: true }),
  ).toBeDisabled();
  expect(posts).toEqual([]);
  await blocker.evaluate(() => globalThis.ownedLegacyConnection.close());
  await expect
    .poll(() => page.evaluate(() => globalThis.ownedAbandonedUpgradeError))
    .toBe("AbortError");
  const afterAbandon = await inspect(page);
  expect(afterAbandon).toEqual(before);
  await storage(page)
    .getByRole("button", { name: "Retry recovery storage", exact: true })
    .click();
  await expect(recovery(page)).toBeVisible();
  const upgraded = await inspect(page);
  expect(upgraded.version).toBe(2);
  expect(row(upgraded, scope).attempt.key).toBe(a.key);
  await stop(page);
  const customers = await rows(page, a.body.name);
  expect(customers).toEqual([]);
  await blocker.close();
  await output(info, "blocked-upgrade.json", {
    name: a.body.name,
    before,
    afterAbandon,
    upgraded,
    abandoned_error: "AbortError",
    posts,
    customers,
  });
});

test("a future database version is retained and refused before recovery or business writes", async ({
  page,
}, info) => {
  const a = attempt(),
    scope = await owner(page),
    initial = await old(page, "encrypted", scope, "read");
  await old(page, "encrypted", scope, "claim", {
    generation: initial.generation,
    attempt: a,
  });
  await page.evaluate(
    () =>
      new Promise((resolve, reject) => {
        const r = indexedDB.open("stateset-freight-save-recovery", 3);
        r.onsuccess = () => {
          r.result.close();
          resolve();
        };
        r.onerror = () => reject(r.error);
      }),
  );
  const before = await inspect(page),
    posts = [];
  page.on("request", (r) => {
    if (business(r)) posts.push(r.url());
  });
  await page.goto("/");
  await expect(storage(page)).toContainText(
    "A newer workspace upgraded browser recovery",
  );
  expect(await inspect(page)).toEqual(before);
  expect(posts).toEqual([]);
  expect(await rows(page, a.body.name)).toEqual([]);
  await output(info, "future-browser-version.json", {
    name: a.body.name,
    before,
    after: await inspect(page),
    posts,
    customers: [],
  });
});

for (const layout of ["missing", "keyPath", "autoIncrement"]) {
  test(`an unsupported ${layout} layout aborts the upgrade without changing stored records`, async ({
    page,
  }, info) => {
    const a = attempt();
    await page.evaluate(
      async ({ layout, a }) => {
        const r = indexedDB.open("stateset-freight-save-recovery", 1);
        r.onupgradeneeded = () => {
          const s = r.result.createObjectStore(
            layout === "missing" ? "owned-unrecognized-store" : "owners",
            layout === "autoIncrement"
              ? { autoIncrement: true }
              : { keyPath: "id" },
          );
          s.put({ id: "owned-layout", original: a });
        };
        await new Promise((resolve, reject) => {
          r.onsuccess = () => {
            r.result.close();
            resolve();
          };
          r.onerror = () => reject(r.error);
        });
      },
      { layout, a },
    );
    const before = await inspect(page),
      posts = [];
    page.on("request", (r) => {
      if (business(r)) posts.push(r.url());
    });
    await page.goto("/");
    await expect(storage(page)).toContainText(
      "layout cannot be upgraded safely",
    );
    expect(await inspect(page)).toEqual(before);
    expect(before.version).toBe(1);
    expect(posts).toEqual([]);
    expect(await rows(page, a.body.name)).toEqual([]);
    await output(info, `unsupported-layout-${layout}.json`, {
      name: a.body.name,
      before,
      after: await inspect(page),
      posts,
      customers: [],
    });
  });
}

test("two new tabs upgrade the same original once and retain its exact request across native admission", async ({
  page,
  context,
}, info) => {
  const a = attempt(),
    scope = await owner(page),
    initial = await old(page, "plain", scope, "read");
  await old(page, "plain", scope, "claim", {
    generation: initial.generation,
    attempt: a,
  });
  const before = await inspect(page);
  const second = await context.newPage();
  await second.clock.install();
  await catalog(second);
  const posts = [];
  for (const tab of [page, second])
    tab.on("request", (r) => {
      if (business(r))
        posts.push({
          key: r.headers()["idempotency-key"],
          body: r.postDataJSON(),
        });
    });
  await Promise.all([page.goto("/"), second.goto("/")]);
  for (const tab of [page, second]) await expect(recovery(tab)).toBeVisible();
  const upgraded = await inspect(page);
  expect(upgraded.version).toBe(2);
  expect(row(upgraded, scope).generation).toBe(initial.generation);
  expect(row(upgraded, scope).attempt.key).toBe(a.key);
  expect(JSON.stringify(upgraded)).not.toContain(a.body.name);
  expect(posts).toEqual([]);
  await recovery(page)
    .getByRole("button", { name: "Check original save", exact: true })
    .click();
  await expect(recovery(page)).toHaveCount(0);
  await expect(recovery(second)).toHaveCount(0);
  const customers = await rows(page, a.body.name);
  expect(customers).toHaveLength(1);
  expect(posts).toEqual([{ key: a.key, body: a.body }]);
  await output(info, "concurrent-browser-upgrade.json", {
    name: a.body.name,
    before,
    upgraded,
    posts,
    customers,
  });
});
