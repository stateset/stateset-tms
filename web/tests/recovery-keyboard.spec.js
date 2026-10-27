const { test, expect } = require("@playwright/test");
const { randomUUID } = require("node:crypto");
const { writeFile } = require("node:fs/promises");

test("recovery dialog keyboard navigation skips held fields and collapsed controls while including its summary", async ({
  page,
}, testInfo) => {
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned credential gateway",
  );
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
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Customers", exact: true })
    .click();
  await page.getByRole("button", { name: "Add customer", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await dialog
    .getByLabel("Customer name", { exact: true })
    .fill(`Owned keyboard ${randomUUID().slice(0, 8)}`);
  await page.route("**/api/freight/customers", (route) =>
    route.fulfill({
      status: 503,
      contentType: "application/json",
      body: JSON.stringify({
        error: {
          message: "Owned uncertain response for keyboard qualification",
        },
      }),
    }),
  );
  await dialog
    .getByRole("button", { name: "Save record", exact: true })
    .click();
  const recovery = page.getByRole("region", {
    name: "Save recovery",
    exact: true,
  });
  await expect(recovery).toContainText("Save outcome not confirmed");
  await expect(
    dialog.getByLabel("Customer name", { exact: true }),
  ).toBeDisabled();
  const summary = recovery.getByText("Stop waiting for this save", {
    exact: true,
  });
  const close = dialog.getByRole("button", {
    name: "Close dialog",
    exact: true,
  });
  await summary.focus();
  const candidates = await dialog.evaluate((root) =>
    [
      ...root.querySelectorAll(
        "button,input,select,textarea,a[href],summary,[tabindex]",
      ),
    ].map((element) => ({
      tag: element.tagName,
      name: element.getAttribute("name"),
      text: element.textContent.slice(0, 60),
      tabIndex: element.tabIndex,
      disabled: element.matches(":disabled"),
      rects: element.getClientRects().length,
      visibility: getComputedStyle(element).visibility,
      checkedVisibility: element.checkVisibility?.(),
      closedDetails: !!element.closest("details:not([open])"),
    })),
  );
  await page.keyboard.press("Tab");
  const active = await page.evaluate(() => ({
    tag: document.activeElement.tagName,
    name: document.activeElement.getAttribute("name"),
    text: document.activeElement.textContent.slice(0, 60),
  }));
  await writeFile(
    testInfo.outputPath("focus-observations.json"),
    JSON.stringify({ candidates, active }, null, 2),
  );
  await expect(close).toBeFocused();
  await page.keyboard.press("Shift+Tab");
  await expect(summary).toBeFocused();
  await page.keyboard.press("Enter");
  await page.keyboard.press("Tab");
  await expect(recovery.getByLabel("Resolution note")).toBeFocused();
  await page.keyboard.press("Tab");
  const resolve = recovery.getByRole("button", {
    name: "Resolve original save",
    exact: true,
  });
  await expect(resolve).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(close).toBeFocused();
  await page.keyboard.press("Shift+Tab");
  await expect(resolve).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  await expect(recovery).toContainText("Save outcome not confirmed");
});
