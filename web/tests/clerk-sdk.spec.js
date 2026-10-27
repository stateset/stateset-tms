const { test, expect } = require("@playwright/test");
const { randomUUID } = require("node:crypto");
const { writeFile } = require("node:fs/promises");
const { installOwnedClerkClient } = require("./clerk-sdk-client-fixture");
const { clerkOrigin, fixtureOrigin } = require("./service-origins.cjs")();
const declared = require("../package.json").dependencies;
const expectedVersion =
  process.env.FREIGHT_E2E_CLERK_SDK_VERSION ||
  declared["@clerk/react"] ||
  declared["@clerk/clerk-react"];
const expectedPackage = expectedVersion.startsWith("5.")
  ? "@clerk/clerk-react"
  : "@clerk/react";

async function control(request, value) {
  expect(new URL(fixtureOrigin).hostname).toBe("127.0.0.1");
  const result = await request.post(`${fixtureOrigin}/__identity/control`, {
    headers: { "x-fixture-key": "freight-fixture-only" },
    data: value,
  });
  expect(result.ok(), await result.text()).toBe(true);
}
async function token(request, claims) {
  const result = await request.post(`${fixtureOrigin}/__identity/token`, {
    headers: { "x-fixture-key": "freight-fixture-only" },
    data: claims,
  });
  expect(result.ok(), await result.text()).toBe(true);
  return (await result.json()).token;
}
async function openSDK(page, request, options = {}) {
  const organization = options.organization || "org_sdk_staff";
  await control(request, {
    status: options.status || "active",
    role: options.partner ? "org:carrier" : "org:operations",
    member: true,
    org: organization,
    outage: false,
  });
  await page.exposeFunction("__ownedClerkToken", (claims) =>
    token(request, claims),
  );
  await page.exposeFunction("__ownedClerkControl", (value) =>
    control(request, value),
  );
  await page.addInitScript(installOwnedClerkClient, {
    organization,
    status: options.status,
  });
  // Only the remote client/UI boundary is controlled. The installed React SDK,
  // actual production Freight components, real JWT gateway and API stay live.
  await page.route("https://**/*", (route) => route.abort());
  await page.goto(clerkOrigin + (options.partner ? "/portal" : "/"));
  await expect
    .poll(() => page.evaluate(() => window.__ownedClerk.calls.loads.length))
    .toBe(1);
  await expect(page.getByRole("status")).toHaveText(
    "Loading StateSet sign-in…",
  );
  expect(await page.evaluate(() => window.__ownedClerk.calls.tokens)).toEqual(
    [],
  );
  await page.evaluate(() => window.__ownedClerk.finishLoad());
}
async function observe(page, info, extra = {}) {
  const calls = await page.evaluate(() => window.__ownedClerk.calls);
  expect(calls.loads).toHaveLength(1);
  expect(calls.loads[0].sdkMetadata).toMatchObject({
    name: expectedPackage,
    version: expectedVersion,
  });
  await writeFile(
    info.outputPath("actual-react-sdk-observations.json"),
    JSON.stringify(
      { expectedPackage, expectedVersion, calls, ...extra },
      null,
      2,
    ),
  );
  return calls;
}
async function journalRows(page) {
  return page.evaluate(
    () =>
      new Promise((resolve, reject) => {
        const opening = indexedDB.open("stateset-freight-save-recovery");
        opening.onerror = () => reject(opening.error);
        opening.onsuccess = () => {
          const db = opening.result;
          const tx = db.transaction("owners", "readonly");
          const rows = [];
          const cursor = tx.objectStore("owners").openCursor();
          cursor.onsuccess = () => {
            const current = cursor.result;
            if (!current) return;
            rows.push({ owner: JSON.parse(current.key), value: current.value });
            current.continue();
          };
          tx.oncomplete = () => {
            db.close();
            resolve(rows);
          };
          tx.onabort = () => {
            db.close();
            reject(tx.error);
          };
        };
      }),
  );
}
test.beforeEach(async () => {
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned remote-client protocol and signed identity fixture",
  );
});
test.afterEach(async ({ request }) => {
  if (process.env.FREIGHT_E2E_EXTERNAL !== "true")
    await control(request, {
      status: "active",
      role: "org:operations",
      member: true,
      org: "org_staff",
      outage: false,
    });
});

for (const partner of [false, true]) {
  test(`actual React SDK loads the ${partner ? "partner" : "staff"} identity boundary and uses the configured sign-out destination`, async ({
    page,
    request,
  }, info) => {
    const errors = [];
    page.on("pageerror", (error) => errors.push(error.message));
    if (partner) await page.setViewportSize({ width: 390, height: 844 });
    const organization = partner ? "org_sdk_carrier" : "org_sdk_staff";
    await openSDK(page, request, { partner, organization });
    if (partner) {
      await expect(page.locator(".partner-header")).toContainText(
        "clerk:user_staff",
      );
      await expect(
        page.getByRole("heading", {
          name: "Carrier offers and shipments",
          exact: true,
        }),
      ).toBeVisible();
    } else {
      await expect(
        page.getByRole("heading", { name: "Overview", exact: true }),
      ).toBeVisible();
      await expect(page.getByLabel("Workspace access role")).toHaveText(
        "Operations access",
      );
    }
    const calls = await observe(page, info, { partner, organization });
    const destination = partner ? "/portal" : "/";
    expect(calls.loads[0].afterSignOutUrl).toBe(destination);
    expect(calls.organizationSwitchers[0]).toMatchObject({
      hidePersonal: true,
      afterSelectOrganizationUrl: destination,
    });
    expect(
      calls.tokens.some(
        (entry) =>
          entry.organizationId === organization &&
          entry.sessionId === "sess_sdk_current",
      ),
    ).toBe(true);
    expect(calls.userButtons).toHaveLength(1);
    expect(calls.userButtons[0].hasAfterSignOutUrl).toBe(
      expectedVersion.startsWith("5."),
    );
    await page
      .getByRole("button", { name: "Owned identity sign out", exact: true })
      .click();
    await expect(
      page.getByRole("heading", {
        name: "Owned identity sign-in boundary",
        exact: true,
      }),
    ).toBeVisible();
    expect(
      await page.evaluate(() => window.__ownedClerk.calls.signOut),
    ).toEqual([{ redirectUrl: destination }]);
    await expect(
      page.locator(partner ? ".partner-workspace" : ".app-shell"),
    ).toHaveCount(0);
    await expect(
      page.getByLabel(partner ? "Partner credential" : "Workspace credential"),
    ).toHaveCount(0);
    expect(errors).toEqual([]);
  });
}

test("actual React SDK organization replacement discards a delayed old workspace and an unsaved old form", async ({
  page,
  request,
}, info) => {
  await page.clock.install();
  await openSDK(page, request);
  await expect(
    page.getByRole("heading", { name: "Overview", exact: true }),
  ).toBeVisible();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Customers", exact: true })
    .click();
  await page.getByRole("button", { name: "Add customer", exact: true }).click();
  const original = `Private old organization draft ${randomUUID()}`;
  await page
    .getByRole("dialog")
    .getByLabel("Customer name", { exact: true })
    .fill(original);
  let release, started;
  const released = new Promise((resolve) => {
    release = resolve;
  });
  const seen = new Promise((resolve) => {
    started = resolve;
  });
  let hold = true;
  await page.route("**/api/freight/workspace?**", async (route) => {
    if (
      hold &&
      route.request().headers()["x-freight-organization-id"] === "org_sdk_staff"
    ) {
      const response = await route.fetch();
      started();
      await released;
      await route.fulfill({ response });
    } else await route.continue();
  });
  await page.clock.fastForward(30000);
  await seen;
  try {
    await page.evaluate(() =>
      window.__ownedClerk.setOrganization("org_sdk_other"),
    );
    await expect(page.locator(".workspace-switch")).toContainText(
      "sdk-identity-other",
    );
    await expect(page.getByRole("dialog")).toHaveCount(0);
    hold = false;
    release();
    await page.unrouteAll({ behavior: "wait" });
    await expect(page.locator(".workspace-switch")).toContainText(
      "sdk-identity-other",
    );
    await expect(page.getByText(original, { exact: true })).toHaveCount(0);
    await observe(page, info, {
      oldForm: original,
      delayedOldWorkspaceDiscarded: true,
    });
  } finally {
    hold = false;
    release();
  }
});

test("actual React SDK token rejection keeps an open staff draft and does not forward a workspace request or sign out", async ({
  page,
  request,
}, info) => {
  await page.clock.install();
  await openSDK(page, request);
  await expect(
    page.getByRole("heading", { name: "Overview", exact: true }),
  ).toBeVisible();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Customers", exact: true })
    .click();
  await page.getByRole("button", { name: "Add customer", exact: true }).click();
  const dialog = page.getByRole("dialog"),
    name = `SDK original draft ${randomUUID()}`;
  await dialog.getByLabel("Customer name", { exact: true }).fill(name);
  const workspaceRequests = [];
  page.on("request", (r) => {
    if (new URL(r.url()).pathname === "/api/freight/workspace")
      workspaceRequests.push(r.url());
  });
  await page.evaluate(() => window.__ownedClerk.setOffline(true));
  await page.clock.fastForward(30000);
  await expect
    .poll(() => page.evaluate(() => window.__ownedClerk.calls.tokenFailures))
    .toBeGreaterThan(0);
  expect(workspaceRequests).toEqual([]);
  await expect(dialog.getByLabel("Customer name", { exact: true })).toHaveValue(
    name,
  );
  await expect(page.getByLabel("Workspace credential")).toHaveCount(0);
  expect(await page.evaluate(() => window.__ownedClerk.calls.signOut)).toEqual(
    [],
  );
  await page.evaluate(() => window.__ownedClerk.setOffline(false));
  const recovered = page.waitForResponse(
    (r) =>
      new URL(r.url()).pathname === "/api/freight/workspace" &&
      r.status() === 200,
  );
  await page.clock.fastForward(30000);
  await (await recovered).finished();
  await expect(dialog.getByLabel("Customer name", { exact: true })).toHaveValue(
    name,
  );
  await observe(page, info, {
    draft: name,
    workspaceRequestsWhileTokenRejected: 0,
    recoveredWithoutSignOut: true,
  });
});

test("actual React SDK pending session cannot mount the staff workspace or forward business reads before activation", async ({
  page,
  request,
}, info) => {
  const requests = [];
  page.on("request", (r) => {
    if (new URL(r.url()).pathname.startsWith("/api/freight/"))
      requests.push(r.url());
  });
  await openSDK(page, request, { status: "pending" });
  await expect(
    page.getByRole("heading", {
      name: "Owned identity sign-in boundary",
      exact: true,
    }),
  ).toBeVisible();
  await expect(page.locator(".app-shell")).toHaveCount(0);
  expect(requests).toEqual([]);
  await page.evaluate(() => window.__ownedClerk.activate());
  await expect(
    page.getByRole("heading", { name: "Overview", exact: true }),
  ).toBeVisible();
  await observe(page, info, {
    pendingSessionForwardedRequests: 0,
    activationLoadedWorkspace: true,
  });
});

test("actual React SDK session replacement clears the ended session's encrypted browser intent", async ({
  page,
  request,
}, info) => {
  await openSDK(page, request);
  await expect(
    page.getByRole("heading", { name: "Overview", exact: true }),
  ).toBeVisible();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Customers", exact: true })
    .click();
  await page.getByRole("button", { name: "Add customer", exact: true }).click();
  const name = `Private ended SDK session ${randomUUID()}`,
    dialog = page.getByRole("dialog");
  await dialog.getByLabel("Customer name", { exact: true }).fill(name);
  await page.route("**/api/freight/customers", (route) =>
    route.fulfill({
      status: 503,
      contentType: "application/json",
      body: JSON.stringify({
        error: { message: "Owned uncertain SDK save response" },
      }),
    }),
  );
  await dialog
    .getByRole("button", { name: "Save record", exact: true })
    .click();
  await expect(
    page.getByRole("region", { name: "Save recovery", exact: true }),
  ).toContainText("Save outcome not confirmed");
  const before = await journalRows(page),
    original = before.find(
      (row) => row.owner[4] === "sess_sdk_current" && row.value.attempt,
    );
  expect(original).toBeTruthy();
  expect(original.value.attempt.sealed.ciphertext).toBeTruthy();
  expect(JSON.stringify(before)).not.toContain(name);
  await page.evaluate(() => window.__ownedClerk.replaceSession());
  await expect(
    page.getByRole("heading", { name: "Overview", exact: true }),
  ).toBeVisible();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect
    .poll(
      async () =>
        (await journalRows(page)).filter(
          (row) => row.owner[4] === "sess_sdk_current" && row.value.attempt,
        ).length,
    )
    .toBe(0);
  const after = await journalRows(page),
    ended = after.find(
      (row) => JSON.stringify(row.owner) === JSON.stringify(original.owner),
    );
  expect(ended.value.generation).not.toBe(original.value.generation);
  await expect(
    page.getByRole("region", { name: "Save recovery", exact: true }),
  ).toHaveCount(0);
  await observe(page, info, {
    before,
    after,
    endedSessionCiphertextCleared: true,
  });
});
