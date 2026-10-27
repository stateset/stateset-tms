const { apiOrigin } = require("./service-origins.cjs")();
const { test, expect } = require("@playwright/test");
const { randomUUID, createHmac, createHash } = require("node:crypto");
const { writeFile } = require("node:fs/promises");

test("first failed authenticated EDI fetch becomes owned work and retains ownership through retry", async ({
  page,
}, testInfo) => {
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned credential services",
  );
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  const credential = "freight-recovery-test-credential-at-least-32-chars";
  const actor =
    "credential:" +
    createHash("sha256").update(credential).digest("hex").slice(0, 12);
  await page.goto("/");
  await page.getByLabel("Workspace credential").fill(credential);
  await page
    .getByRole("button", { name: "Open workspace", exact: true })
    .click();
  await expect(
    page.getByRole("heading", { name: "Overview", exact: true }),
  ).toBeVisible();
  const documentId = `missing-${randomUUID()}`,
    delivery = randomUUID();
  const notification = {
    type: "document.recorded",
    tenant_id: "recovery",
    document_id: documentId,
    partner_id: "recovery-carrier",
    reference: documentId,
    transaction_set: "990",
    direction: "inbound",
    status: "accepted",
  };
  const body = JSON.stringify(notification),
    timestamp = String(Math.floor(Date.now() / 1000));
  const signature = createHmac(
    "sha256",
    "freight-recovery-callback-secret-at-least-32-chars",
  )
    .update(`${timestamp}.${delivery}.${body}`)
    .digest("hex");
  const response = await page.request.post(
    `${apiOrigin}/webhooks/edi/recovery`,
    {
      headers: {
        "Content-Type": "application/json",
        "x-stateset-key-version": "1",
        "x-stateset-event-id": delivery,
        "x-stateset-timestamp": timestamp,
        "x-stateset-signature": `v1=${signature}`,
      },
      data: body,
    },
  );
  expect(response.ok(), await response.text()).toBe(true);
  const read = async (path) => {
    const result = await page.request.get(`/api/freight/${path}`);
    expect(result.ok(), await result.text()).toBe(true);
    return result.json();
  };
  const inbox = async () =>
    (await read("workspace")).inbox.find(
      (job) => job.document_id === documentId,
    );
  await expect
    .poll(async () => (await inbox())?.status, { timeout: 10000 })
    .toBe("retry");
  const job = await inbox();
  const queue = await read("operations?view=active");
  const issue = queue.issues.find(
    (row) =>
      row.signal.kind === "inbound_failure" && row.signal.subject_id === job.id,
  );
  expect(issue).toBeTruthy();
  const initial = await read(`operations/issues/${issue.id}`);
  // History proves detection on attempt one even if a subsequent worker tick runs.
  expect(initial.case.history[0].source.source.total_attempts).toBe(1);
  expect(initial.case.status).toBe("open");
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Control tower", exact: true })
    .click();
  const card = page.getByRole("article", {
    name: "Inbound document needs recovery · Integration",
    exact: true,
  });
  await expect(card).toBeVisible();
  await card
    .getByRole("button", { name: "Take ownership", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  await dialog
    .getByLabel("Next follow-up (UTC)")
    .fill(new Date(Date.now() + 90 * 60000).toISOString().slice(0, 16));
  await dialog
    .getByLabel("Follow-up findings and next step")
    .fill(
      "Inspect the missing canonical EDI document with the integration operator.",
    );
  await dialog.getByRole("checkbox").check();
  await dialog
    .getByRole("button", { name: "Record follow-up action", exact: true })
    .click();
  await expect(dialog).toHaveCount(0);
  await expect(card).toContainText(actor);
  const owned = await read(`operations/issues/${issue.id}`);
  const retry = await page.request.post(`/api/freight/inbox/${job.id}/retry`, {
    headers: {
      Origin: new URL(page.url()).origin,
      "Idempotency-Key": randomUUID(),
    },
    data: {},
  });
  expect(retry.ok(), await retry.text()).toBe(true);
  const after = await read(`operations/issues/${issue.id}`);
  expect(after.id).toBe(issue.id);
  expect(after.case.owner).toBe(actor);
  expect(after.case.follow_up_at).toBe(owned.case.follow_up_at);
  expect(after.case.first_detected_at).toBe(initial.case.first_detected_at);
  expect(after.case.cycle).toBe(1);
  expect(after.source_active).toBe(true);
  expect(after.case.history.slice(0, owned.case.history.length)).toEqual(
    owned.case.history,
  );
  const retained = await inbox();
  expect(retained.original_body).toBe(body);
  expect(retained.notification).toEqual(notification);
  expect(errors).toEqual([]);
  await writeFile(
    testInfo.outputPath("first-failure-proof.json"),
    JSON.stringify(
      {
        documentId,
        issueId: issue.id,
        firstAttempt: initial.case.history[0].source.source,
        owner: after.case.owner,
        cycle: after.case.cycle,
        sourceActive: after.source_active,
        originalBodyRetained: true,
        pageErrors: errors,
      },
      null,
      2,
    ),
  );
});
