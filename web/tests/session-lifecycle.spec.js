const { test, expect } = require("@playwright/test");
const credential = "freight-test-quote-source-credential-at-least-32-chars";

async function signIn(page) {
  await page.getByLabel("Workspace credential").fill(credential);
  await page.getByRole("button", { name: "Open workspace" }).click();
  await expect(
    page.getByRole("heading", { name: "Overview", exact: true }),
  ).toBeVisible();
}

test.beforeEach(async ({ page }) => {
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires the owned credential gateway and synthetic Sync fixture",
  );
  await page.clock.install();
  await page.goto("/");
  await signIn(page);
});

test("staff access denial fences an older successful refresh until explicit sign-in", async ({
  page,
}) => {
  const workspace = await (
    await page.request.get("/api/freight/workspace")
  ).json();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Load board", exact: false })
    .click();
  await page.getByRole("button", { name: "Create load", exact: true }).click();
  await expect(page.getByRole("dialog")).toBeVisible();
  await page.keyboard.press("Escape");
  let started,
    count = 0;
  const held = new Promise((resolve) => {
    started = resolve;
  });
  await page.route(
    "**/api/freight/workspace?history=deferred",
    async (route) => {
      if (++count === 1) return started(route);
      await route.fulfill({
        status: 401,
        contentType: "application/json",
        body: JSON.stringify({
          error: { message: "A current staff session is required." },
        }),
      });
    },
  );
  await page.getByRole("button", { name: "Refresh load board" }).click();
  const older = await held;
  // Keep an unfinished form open while the newer automatic refresh is denied.
  await page.getByRole("button", { name: "Create load", exact: true }).click();
  const denied = page.waitForResponse(
    (r) =>
      new URL(r.url()).pathname === "/api/freight/workspace" &&
      r.status() === 401,
  );
  await page.clock.fastForward(30000);
  await (await denied).finished();
  await expect(page.getByLabel("Workspace credential")).toBeVisible();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  const late = page.waitForResponse(
    (r) =>
      new URL(r.url()).pathname === "/api/freight/workspace" &&
      r.status() === 200,
  );
  await older.fulfill({
    status: 200,
    contentType: "application/json",
    body: JSON.stringify(workspace),
  });
  await (await late).finished();
  await page.clock.runFor(100);
  await expect(page.getByLabel("Workspace credential")).toBeVisible();
  await expect(page.locator(".app-shell")).toHaveCount(0);
  await page.clock.fastForward(60000);
  expect(count).toBe(2);
  await page.unroute("**/api/freight/workspace?history=deferred");
  await signIn(page);
});

test("staff sign-out closes immediately and fences delayed Sync reads across the next session", async ({
  page,
}) => {
  await page.getByRole("button", { name: "Integrations", exact: true }).click();
  let sourceStarted, deleteStarted;
  const sourceHeld = new Promise((resolve) => {
    sourceStarted = resolve;
  });
  const deleteHeld = new Promise((resolve) => {
    deleteStarted = resolve;
  });
  await page.route("**/api/freight/sync/orders", (route) =>
    sourceStarted(route),
  );
  await page.route("**/api/session", (route) =>
    route.request().method() === "DELETE"
      ? deleteStarted(route)
      : route.continue(),
  );
  await page.getByRole("button", { name: "Import sync order" }).click();
  const source = await sourceHeld;
  await page.getByRole("button", { name: /Operations team.*Sign out/ }).click();
  const deleting = await deleteHeld;
  await expect(page.getByLabel("Workspace credential")).toBeVisible();
  await expect(page.locator(".app-shell")).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Signing out…" }),
  ).toBeDisabled();
  let backgroundReads = 0;
  await page.route("**/api/freight/workspace?history=deferred", (route) => {
    backgroundReads++;
    return route.continue();
  });
  await page.clock.fastForward(60000);
  expect(backgroundReads).toBe(0);
  await deleting.fulfill({
    status: 503,
    contentType: "application/json",
    body: "{}",
  });
  await expect(
    page.getByRole("button", { name: "Retry sign out", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Open workspace" }),
  ).toBeDisabled();
  await page.unroute("**/api/session");
  await page
    .getByRole("button", { name: "Retry sign out", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "Open workspace" }),
  ).toBeEnabled();
  expect((await page.request.get("/api/freight/workspace")).status()).toBe(401);
  await signIn(page);
  const currentReads = backgroundReads;
  const late = page.waitForResponse(
    (r) => r.url().endsWith("/api/freight/sync/orders") && r.status() === 200,
  );
  await source.fulfill({
    status: 200,
    contentType: "application/json",
    body: JSON.stringify({
      items: [
        {
          id: "old-private-order",
          orderNumber: "OLD-SESSION-PRIVATE-ORDER",
          status: "pending",
          lineItems: [],
        },
      ],
      pagination: { hasNext: false },
    }),
  });
  await (await late).finished();
  await page.clock.runFor(100);
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(page.getByText("OLD-SESSION-PRIVATE-ORDER")).toHaveCount(0);
  await expect(
    page.getByRole("heading", { name: "Overview", exact: true }),
  ).toBeVisible();
  expect(backgroundReads).toBe(currentReads);
});
