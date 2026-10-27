const { test, expect } = require("@playwright/test");
const { writeFile } = require("node:fs/promises");
const { randomUUID } = require("node:crypto");

for (const viewport of [
  { width: 1280, height: 900 },
  { width: 390, height: 844 },
]) {
  test(`staff dialog isolates background controls and restores its opener at ${viewport.width}px`, async ({
    page,
  }, testInfo) => {
    await page.setViewportSize(viewport);
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
    if (viewport.width < 761)
      await page
        .getByRole("button", { name: "Toggle navigation", exact: true })
        .click();
    await page
      .getByRole("navigation")
      .getByRole("button", { name: "Customers", exact: true })
      .click();
    const opener = page.getByRole("button", {
      name: "Add customer",
      exact: true,
    });
    await opener.click();
    const dialog = page.getByRole("dialog", {
      name: "Add customer",
      exact: true,
    });
    await expect(dialog).toBeVisible();
    const background = page.locator(
      'button[aria-label="View shipment exceptions"]',
    );
    const observation = await background.evaluate((element) => {
      element.focus();
      return {
        backgroundAcceptedFocus: document.activeElement === element,
        backgroundHasInertAncestor: !!element.closest("[inert]"),
        focusedElementInsideDialog:
          !!document.activeElement.closest('[role="dialog"]'),
      };
    });
    observation.backgroundAccessibleButtons = await page
      .getByRole("button", { name: "View shipment exceptions", exact: true })
      .count();
    await writeFile(
      testInfo.outputPath("modal-isolation-observation.json"),
      JSON.stringify(observation, null, 2),
    );
    expect.soft(observation.backgroundAcceptedFocus).toBe(false);
    expect.soft(observation.backgroundHasInertAncestor).toBe(true);
    expect.soft(observation.backgroundAccessibleButtons).toBe(0);
    const close = dialog.getByRole("button", {
      name: "Close dialog",
      exact: true,
    });
    await close.focus();
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
    await expect(opener).toBeFocused();
    await expect(background).toBeVisible();
    await expect(
      page.getByRole("button", {
        name: "View shipment exceptions",
        exact: true,
      }),
    ).toHaveCount(1);
    await background.focus();
    await expect(background).toBeFocused();
    expect(await page.locator("[inert]").count()).toBe(0);
    expect(await page.evaluate(() => document.body.style.overflow)).not.toBe(
      "hidden",
    );
  });
}

test("nested carrier confirmation isolates its parent and restores existing and newly mounted background controls", async ({
  page,
}, testInfo) => {
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
  const origin = new URL(page.url()).origin;
  const post = async (path, body) => {
    const response = await page.request.post(`/api/freight/${path}`, {
      headers: { Origin: origin, "Idempotency-Key": randomUUID() },
      data: body,
    });
    expect(response.ok(), await response.text()).toBe(true);
    return (await response.json()).data;
  };
  const day = new Date(Date.now() + 86400000).toISOString().slice(0, 10);
  const reference = `MODAL-${randomUUID().slice(0, 8)}`;
  const created = await post("loads", {
    reference,
    customer_id: "c1",
    mode: "FTL",
    equipment: "Dry van",
    commodity: "Owned dialog qualification freight",
    weight_lbs: 1000,
    miles: 173,
    buy_cents: 125001,
    sell_cents: 150001,
    origin: {
      facility: "Owned Portland origin",
      city: "Portland",
      state: "OR",
      date: day,
      address_lines: ["15 Dock Street"],
      postal_code: "97201",
      country: "US",
    },
    destination: {
      facility: "Owned Seattle receiver",
      city: "Seattle",
      state: "WA",
      date: day,
      address_lines: ["25 Receiving Ave"],
      postal_code: "98101",
      country: "US",
    },
  });
  const snapshot = async () =>
    (await page.request.get("/api/freight/workspace")).json();
  let workspace = await snapshot();
  let load = workspace.loads.find((row) => row.id === created.id);
  await post(`loads/${created.id}/commands`, {
    action: "tender",
    expected_version: load.version,
    carrier_id: "ca4",
  });
  workspace = await snapshot();
  load = workspace.loads.find((row) => row.id === created.id);
  await page.reload();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /^Load board\b/ })
    .click();
  await page.evaluate(() => {
    document.querySelector(".app-shell").setAttribute("aria-hidden", "false");
    const canary = document.createElement("button");
    canary.id = "owned-prior-inert-control";
    canary.textContent = "Owned previously hidden control";
    canary.setAttribute("inert", "");
    canary.setAttribute("aria-hidden", "true");
    document.body.append(canary);
  });
  const opener = page.getByRole("button", { name: reference, exact: true });
  await opener.click();
  const parent = page
    .locator('[role="dialog"]')
    .filter({ has: page.locator(`h2`, { hasText: reference }) });
  await expect(parent).toBeVisible();
  const issue = parent.getByRole("button", {
    name: "Issue carrier confirmation",
    exact: true,
  });
  await issue.click();
  const child = page.getByRole("dialog", {
    name: "Issue carrier rate confirmation",
    exact: true,
  });
  await expect(child).toBeVisible();
  await expect(parent).toHaveAttribute("aria-modal", "false");
  await expect(child).toHaveAttribute("aria-modal", "true");
  const parentClose = parent
    .locator(".modal-header")
    .first()
    .locator('button[aria-label="Close dialog"]');
  expect(
    await parentClose.evaluate((element) => {
      element.focus();
      return document.activeElement === element;
    }),
  ).toBe(false);
  await expect(
    child.getByRole("button", { name: "Close dialog", exact: true }),
  ).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(child).toHaveCount(0);
  await expect(parent).toHaveAttribute("aria-modal", "true");
  await expect(issue).toBeFocused();
  await expect(page.locator(".app-shell")).toHaveAttribute("inert", "");
  await page.evaluate(() => {
    const late = document.createElement("button");
    late.id = "owned-late-background-control";
    late.textContent = "Owned later background control";
    document.body.append(late);
  });
  const late = page.locator("#owned-late-background-control");
  await expect(late).toHaveAttribute("inert", "");
  await expect(late).toHaveAttribute("aria-hidden", "true");
  expect(
    await late.evaluate((element) => {
      element.focus();
      return document.activeElement === element;
    }),
  ).toBe(false);
  await page.keyboard.press("Escape");
  await expect(parent).toHaveCount(0);
  await expect(opener).toBeFocused();
  await expect(page.locator(".app-shell")).toHaveAttribute(
    "aria-hidden",
    "false",
  );
  expect(await page.locator(".app-shell").getAttribute("inert")).toBe(null);
  await expect(page.locator("#owned-prior-inert-control")).toHaveAttribute(
    "inert",
    "",
  );
  await expect(page.locator("#owned-prior-inert-control")).toHaveAttribute(
    "aria-hidden",
    "true",
  );
  expect(await late.getAttribute("inert")).toBe(null);
  expect(await late.getAttribute("aria-hidden")).toBe(null);
  await late.focus();
  await expect(late).toBeFocused();
  expect(await page.evaluate(() => document.body.style.overflow)).not.toBe(
    "hidden",
  );
  const after = await snapshot();
  expect(after.loads.find((row) => row.id === created.id).version).toBe(
    load.version,
  );
  expect(
    after.rate_confirmations.filter((row) => row.load_id === created.id),
  ).toHaveLength(0);
  await writeFile(
    testInfo.outputPath("nested-modal-observation.json"),
    JSON.stringify(
      {
        createdLoad: created.id,
        unchangedLoadVersion: load.version,
        carrierConfirmationNotIssued: true,
        nestedEscapeRestoresParent: true,
        originalAttributesRestored: true,
        lateBackgroundRestored: true,
      },
      null,
      2,
    ),
  );
});
