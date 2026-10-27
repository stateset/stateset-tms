const { test, expect } = require("@playwright/test");
const { execFileSync } = require("node:child_process");
const { randomUUID } = require("node:crypto");
const path = require("node:path");

test("independent invoice review binds the displayed revision across staff sessions and mobile approval", async ({
  page,
  browser,
}) => {
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true" ||
      !process.env.FREIGHT_E2E_DATABASE_URL,
    "requires owned PostgreSQL browser services",
  );
  test.setTimeout(90000);
  execFileSync(
    process.env.FREIGHT_E2E_INVOICE_FIXTURE ||
      path.join(
        __dirname,
        "../../target/debug/examples/invoice_review_fixture",
      ),
    ["browser-invoice-review"],
    { env: process.env },
  );
  const origin = new URL(test.info().project.use.baseURL).origin;
  async function signIn(p, role) {
    await p.goto("/");
    await p
      .getByLabel("Workspace credential")
      .fill(`freight-test-invoice-${role}-credential-at-least-32-chars`);
    await p
      .getByRole("button", { name: "Open workspace", exact: true })
      .click();
    await expect(
      p.getByRole("heading", { name: "Overview", exact: true }),
    ).toBeVisible();
    await audit(p);
  }
  async function audit(p) {
    const menu = p.getByRole("button", {
      name: "Toggle navigation",
      exact: true,
    });
    if (await menu.isVisible()) await menu.click();
    await p
      .getByRole("navigation")
      .getByRole("button", { name: "Freight audit", exact: true })
      .click();
  }
  const snapshot = async (p) =>
    (await p.request.get("/api/freight/workspace")).json();
  async function post(p, route, body) {
    return p.request.post(`/api/freight/${route}`, {
      headers: { Origin: origin, "Idempotency-Key": randomUUID() },
      data: body,
    });
  }
  await signIn(page, "maker");
  let w = await snapshot(page);
  const invoice = w.invoices.find((v) => v.status === "matched");
  const row = (p) => p.getByRole("row").filter({ hasText: invoice.number });
  async function reviewEntry() {
    await row(page)
      .getByRole("button", { name: "Review entry", exact: true })
      .click();
    const d = page.getByRole("dialog");
    await expect(d).toContainText(invoice.number);
    await d
      .getByLabel("Carrier invoice evidence reference")
      .fill("Owned carrier source INV-1180");
    await d
      .getByLabel("Entry review findings")
      .fill(
        "Verified actual invoice, assigned carrier and exact agreed amount",
      );
    await d.getByRole("button", { name: "Save record", exact: true }).click();
    await expect(d).toHaveCount(0);
    await expect(
      row(page).getByRole("button", {
        name: "Independent approval required",
        exact: true,
      }),
    ).toBeDisabled();
  }
  await reviewEntry();
  const context = await browser.newContext({
    viewport: { width: 390, height: 844 },
  });
  const checker = await context.newPage();
  let detailRequests = 0;
  checker.on("request", (request) => {
    if (new URL(request.url()).pathname.endsWith("/approval-evidence"))
      detailRequests++;
  });
  try {
    await signIn(checker, "checker");
    await row(checker)
      .getByRole("button", { name: "Approve", exact: true })
      .click();
    let dialog = checker.getByRole("dialog");
    await expect(dialog).toContainText("browser:invoice-maker");
    await dialog
      .getByLabel("Independent approval findings")
      .fill(
        "Independently verified invoice, agreed charges and actual delivery proof",
      );
    w = await snapshot(page);
    const load = w.loads.find((v) => v.id === invoice.load_id);
    const changed = await post(page, `loads/${load.id}/pod/review`, {
      expected_version: load.version,
      decision: "accepted",
      note: "Retained a new actual finance inspection of the external receiver evidence",
    });
    expect(changed.ok(), await changed.text()).toBe(true);
    // Hold the claim notification's decrypted original until the known
    // rejection clears it. A stale read must not resurrect save recovery.
    let releaseOldRead, opened, finishedRead;
    const oldReadHeld = new Promise((resolve) => {
      releaseOldRead = resolve;
    });
    const openStarted = new Promise((resolve) => {
      opened = resolve;
    });
    const openFinished = new Promise((resolve) => {
      finishedRead = resolve;
    });
    const holdOldRead = async (route) => {
      const body = route.request().postDataJSON();
      if (body.action !== "open") return route.continue();
      const response = await route.fetch();
      opened();
      await oldReadHeld;
      await route.fulfill({ response });
      finishedRead();
    };
    const rejectAfterRead = async (route) => {
      const response = await route.fetch();
      await openStarted;
      await route.fulfill({ response });
    };
    await checker.route("**/api/freight/save-journal", holdOldRead);
    await checker.route(
      `**/api/freight/invoices/${invoice.id}/approve`,
      rejectAfterRead,
    );
    await dialog
      .getByRole("button", { name: "Save record", exact: true })
      .click();
    await expect(dialog.getByRole("alert")).toContainText("Invoice changed;");
    releaseOldRead();
    await openFinished;
    await checker.evaluate(
      () =>
        new Promise((resolve) =>
          requestAnimationFrame(() => requestAnimationFrame(resolve)),
        ),
    );
    await expect(
      checker.getByRole("region", { name: "Save recovery", exact: true }),
    ).toHaveCount(0);
    await expect(dialog.getByRole("alert")).toContainText("Invoice changed;");
    await checker.unroute("**/api/freight/save-journal", holdOldRead);
    await checker.unroute(
      `**/api/freight/invoices/${invoice.id}/approve`,
      rejectAfterRead,
    );
    expect(
      (await snapshot(page)).invoices.find((v) => v.id === invoice.id).status,
    ).toBe("matched");
    await checker.keyboard.press("Escape");
    await page.reload();
    await audit(page);
    await reviewEntry();
    await checker.reload();
    await audit(checker);
    await row(checker)
      .getByRole("button", { name: "Approve", exact: true })
      .click();
    dialog = checker.getByRole("dialog");
    await dialog
      .getByLabel("Independent approval findings")
      .fill(
        "Fresh independent invoice and delivery review after the evidence update",
      );
    await dialog
      .getByRole("button", { name: "Save record", exact: true })
      .click();
    await expect(dialog).toHaveCount(0);
    await expect(
      row(checker).getByRole("cell", { name: "Approved", exact: true }),
    ).toBeVisible();
    const viewApproval = row(checker).getByRole("button", {
      name: "View approval",
      exact: true,
    });
    const compactResponse = await checker.request.get(
      "/api/freight/workspace?history=deferred",
      { headers: { "X-Freight-Approval-Details": "deferred" } },
    );
    expect(compactResponse.ok()).toBe(true);
    const compactInvoice = (await compactResponse.json()).invoices.find(
      (v) => v.id === invoice.id,
    );
    for (const field of [
      "entry_review_note",
      "approval_note",
      "evidence_reference",
    ])
      expect(
        compactInvoice.approval_control.approval_evidence[field],
      ).toBeUndefined();
    expect(detailRequests).toBe(0);
    let releaseFirst, firstStarted, firstFinished;
    const heldFirst = new Promise((resolve) => {
      releaseFirst = resolve;
    });
    const firstSeen = new Promise((resolve) => {
      firstStarted = resolve;
    });
    const firstDone = new Promise((resolve) => {
      firstFinished = resolve;
    });
    let detailMode = "hold";
    const detailRoute = `**/api/freight/invoices/${invoice.id}/approval-evidence`;
    await checker.route(detailRoute, async (route) => {
      if (detailMode === "unavailable") {
        detailMode = "normal";
        return route.fulfill({
          status: 503,
          contentType: "application/json",
          body: JSON.stringify({
            error: { message: "Owned approval read temporarily unavailable" },
          }),
        });
      }
      const response = await route.fetch();
      if (detailMode === "hold") {
        detailMode = "normal";
        const body = await response.json();
        body.approval_evidence.entry_review_note =
          "Stale read must not replace a newly opened approval record";
        firstStarted();
        await heldFirst;
        await route.fulfill({ response, json: body });
        firstFinished();
        return;
      }
      if (detailMode === "wrong-invoice") {
        detailMode = "normal";
        const body = await response.json();
        body.invoice_id = "another-owned-invoice";
        return route.fulfill({ response, json: body });
      }
      if (detailMode === "wrong-decision") {
        detailMode = "normal";
        const body = await response.json();
        body.approval_evidence.approval_event_id = "another-owned-decision";
        return route.fulfill({ response, json: body });
      }
      return route.fulfill({ response });
    });
    await expect(viewApproval).toBeVisible();
    await viewApproval.click();
    const record = checker.getByRole("dialog", {
      name: `Invoice ${invoice.number} approval`,
      exact: true,
    });
    await firstSeen;
    await expect(record.getByRole("status")).toHaveText(
      "Loading approval record…",
    );
    await checker.keyboard.press("Escape");
    await expect(record).toHaveCount(0);
    await expect(viewApproval).toBeFocused();
    await viewApproval.click();
    await expect(record).toContainText("browser:invoice-maker");
    await expect(record).toContainText("browser:invoice-checker");
    await expect(record).toContainText("Owned carrier source INV-1180");
    await expect(record).toContainText(
      "Fresh independent invoice and delivery review after the evidence update",
    );
    releaseFirst();
    await firstDone;
    await checker.evaluate(
      () =>
        new Promise((resolve) =>
          requestAnimationFrame(() => requestAnimationFrame(resolve)),
        ),
    );
    await expect(record).not.toContainText("Stale read must not replace");
    await expect(
      record.getByRole("button", { name: "Save record" }),
    ).toHaveCount(0);
    await expect(record.locator("time")).toHaveCount(2);
    await checker.screenshot({
      path: path.join(test.info().outputDir, "approval-record-mobile.png"),
      fullPage: true,
    });
    await checker.keyboard.press("Escape");
    await expect(record).toHaveCount(0);
    await expect(viewApproval).toBeFocused();
    for (const [mode, message] of [
      ["wrong-invoice", "does not match this invoice"],
      ["wrong-decision", "changed or is unavailable"],
      ["unavailable", "temporarily unavailable"],
    ]) {
      detailMode = mode;
      await viewApproval.click();
      await expect(record.getByRole("alert")).toContainText(message);
      await expect(record.locator("time")).toHaveCount(0);
      await record
        .getByRole("button", { name: "Retry approval record", exact: true })
        .click();
      await expect(record).toContainText("browser:invoice-checker");
      await checker.keyboard.press("Escape");
    }
    const saved = await snapshot(checker);
    const approvedControl = saved.invoices.find(
      (v) => v.id === invoice.id,
    ).approval_control;
    expect(approvedControl.entry_reviewed_by).toBe("browser:invoice-maker");
    expect(approvedControl.approval_evidence.approved_by).toBe(
      "browser:invoice-checker",
    );
    const laterLoad = saved.loads.find((v) => v.id === invoice.load_id);
    const laterInspection = await post(
      page,
      `loads/${laterLoad.id}/pod/review`,
      {
        expected_version: laterLoad.version,
        decision: "accepted",
        note: "Later receiver inspection must retain the original invoice approval record",
      },
    );
    expect(laterInspection.ok(), await laterInspection.text()).toBe(true);
    await checker.reload();
    await audit(checker);
    const retained = await snapshot(checker);
    expect(
      retained.invoices.find((v) => v.id === invoice.id).approval_control
        .approval_evidence,
    ).toEqual(approvedControl.approval_evidence);
    await row(checker)
      .getByRole("button", { name: "View approval", exact: true })
      .click();
    await expect(checker.getByRole("dialog")).toContainText(
      "browser:invoice-maker",
    );
    await checker.keyboard.press("Escape");
    const historical = saved.invoices.find(
      (v) => v.id !== invoice.id && v.status === "approved",
    );
    expect(historical.approval_control.approval_evidence).toBeNull();
    expect(historical.approval_control.entry_reviewed_by).toBeNull();
    const historicalRow = checker
      .getByRole("row")
      .filter({ hasText: historical.number });
    await expect(historicalRow).toContainText("Review details unavailable");
    await expect(
      historicalRow.getByRole("button", { name: "View approval" }),
    ).toHaveCount(0);
    const decisions = saved.events.filter(
      (e) => e.entity_id === invoice.id && e.kind === "invoice.approve",
    );
    expect(decisions).toHaveLength(1);
    expect(decisions[0].actor).toBe("browser:invoice-checker");
    expect(decisions[0].details.entry_review_id).toBeTruthy();
    const journal = saved.journal.filter((e) => e.source_id === invoice.id);
    expect(journal).toHaveLength(1);
    expect(journal[0].actor).toBe("browser:invoice-checker");
    expect(
      await checker.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBe(true);
    const switched = await checker.request.post("/api/session", {
      headers: { Origin: origin },
      data: { token: "freight-test-viewer-credential-at-least-32-chars" },
    });
    expect(switched.ok(), await switched.text()).toBe(true);
    const denied = checker.waitForResponse(
      (response) =>
        new URL(response.url()).pathname.endsWith("/approval-evidence") &&
        response.status() === 409,
    );
    await row(checker)
      .getByRole("button", { name: "View approval", exact: true })
      .click();
    expect((await (await denied).json()).error.code).toBe(
      "workspace_session_changed",
    );
    await expect(checker.getByLabel("Workspace credential")).toBeVisible();
    await expect(checker.getByRole("dialog")).toHaveCount(0);
  } finally {
    await context.close();
  }
});
