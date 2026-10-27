const { clerkOrigin, apiOrigin, fixtureOrigin } =
  require("./service-origins.cjs")();
const { test, expect } = require("@playwright/test");
const { createHmac, createHash, randomUUID } = require("node:crypto");
const { readFile } = require("node:fs/promises");
const credential = "freight-test-operator-credential-at-least-32-chars";
test.beforeEach(async ({ page }) => {
  if (process.env.FREIGHT_E2E_EXTERNAL === "true") return;
  await page.goto("/");
  await page.getByLabel("Workspace credential").fill(credential);
  await page.getByRole("button", { name: "Open workspace" }).click();
  await expect(
    page.getByRole("heading", { name: "Overview", exact: true }),
  ).toBeVisible();
});

test("quotes retain carrier procurement and customer acceptance through booking release and replacement", async ({
  page,
  browser,
}, testInfo) => {
  test.setTimeout(90000);
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned quote workspace",
  );
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  const origin = new URL(page.url()).origin,
    reference = `Q-E2E-${Date.now()}`;
  const snapshot = async () => {
    const r = await page.request.get("/api/freight/workspace");
    expect(r.ok()).toBe(true);
    return r.json();
  };
  const post = async (path, body) => {
    const r = await page.request.post(`/api/freight/${path}`, {
      headers: { Origin: origin, "Idempotency-Key": randomUUID() },
      data: body,
    });
    expect(r.ok(), await r.text()).toBe(true);
    return (await r.json()).data;
  };
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Quotes", exact: true })
    .click();
  await page.getByRole("button", { name: "Create quote", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Quote reference", { exact: true }).fill(reference);
  await dialog
    .getByLabel("Quoted customer", { exact: true })
    .selectOption("c1");
  await dialog.getByLabel("Quoted commodity").fill("Quoted consumer goods");
  for (const [prefix, city, state, street, postal] of [
    ["pickup", "Portland", "OR", "15 Dock Street", "97201"],
    ["delivery", "Seattle", "WA", "25 Receiving Ave", "98101"],
  ]) {
    await dialog.getByLabel(`Quote ${prefix} facility`).fill(`${city} DC`);
    await dialog.getByLabel(`Quote ${prefix} street`).fill(street);
    await dialog.getByLabel(`Quote ${prefix} city`).fill(city);
    await dialog.getByLabel(`Quote ${prefix} state`).fill(state);
    await dialog.getByLabel(`Quote ${prefix} postal code`).fill(postal);
    await dialog
      .getByLabel(`Quote ${prefix} date`)
      .fill(new Date(Date.now() + 2 * 86400000).toISOString().slice(0, 10));
  }
  await dialog.getByLabel("Customer quote price").fill("1500.01");
  await dialog.getByRole("button", { name: "Save quote action" }).click();
  await expect(dialog).not.toBeVisible();
  const details = page.getByRole("region", {
    name: `Quote details ${reference}`,
  });
  await expect(details).toBeVisible();
  const proof = async (ref) => {
    await dialog
      .getByLabel("Quote counterparty contact")
      .fill("Verified transport contact");
    await dialog.getByLabel("Quote confirmation reference").fill(ref);
    await dialog
      .getByLabel("Quote evidence note")
      .fill("Actual synthetic original correspondence retained");
  };
  await details
    .getByRole("button", { name: "Record carrier offer", exact: true })
    .click();
  await dialog.getByLabel("Quoted carrier").selectOption("ca4");
  await dialog.getByLabel("Actual carrier offer (USD)").fill("1250.01");
  await proof("PRIVATE-CARRIER-OFFER");
  await dialog.getByRole("button", { name: "Save quote action" }).click();
  await expect(dialog).not.toBeVisible();
  await details.getByRole("button", { name: "Issue customer quote" }).click();
  let w = await snapshot(),
    quote = w.quotes.find((q) => q.reference === reference);
  await dialog
    .getByLabel("Selected carrier offer")
    .selectOption(quote.revisions[0].offers[0].id);
  await dialog.getByRole("button", { name: "Save quote action" }).click();
  await expect(dialog).not.toBeVisible();
  const download = page.waitForEvent("download");
  await details
    .getByRole("link", { name: "Download customer quote", exact: true })
    .click();
  const file = await download,
    html = (await readFile(await file.path())).toString();
  expect(html).toContain("1500.01");
  expect(html).not.toContain("1250.01");
  expect(html).not.toContain("PRIVATE-CARRIER-OFFER");
  expect(html).not.toContain("Blue Ridge");
  await details
    .getByRole("button", { name: "Record customer acceptance", exact: true })
    .click();
  await proof("CUSTOMER-ACCEPTED");
  await dialog.getByRole("button", { name: "Save quote action" }).click();
  await expect(dialog).not.toBeVisible();
  const before = await snapshot();
  await details.getByRole("button", { name: "Book quoted shipment" }).click();
  await dialog.getByRole("button", { name: "Save quote action" }).click();
  await expect(dialog).not.toBeVisible();
  w = await snapshot();
  quote = w.quotes.find((q) => q.reference === reference);
  const load_id = quote.booked_load_id;
  expect(w.loads.find((l) => l.id === load_id)).toMatchObject({
    status: "planned",
    carrier_id: null,
    buy_cents: 125001,
    sell_cents: 150001,
  });
  expect(w.journal.length).toBe(before.journal.length);
  expect(w.outbox.length).toBe(before.outbox.length);
  await details.getByRole("button", { name: "Open booked shipment" }).click();
  await expect(
    dialog.getByRole("button", { name: "Edit plan", exact: true }),
  ).toHaveCount(0);
  await dialog.getByRole("button", { name: "Tender", exact: true }).click();
  await dialog.getByLabel("Carrier", { exact: true }).selectOption("ca4");
  await dialog
    .getByRole("button", { name: "Confirm tender", exact: true })
    .click();
  await expect(dialog).not.toBeVisible();
  await post(`loads/${load_id}/commands`, {
    action: "accept",
    expected_version: 2,
  });
  const release = await post(`loads/${load_id}/release-requests`, {
    expected_load_version: 3,
    disposition: "replan",
    reason: "Original carrier cannot cover",
  });
  await post(`release-requests/${release.id}/confirm`, {
    expected_load_version: 4,
    carrier_contact: "Original carrier dispatch",
    confirmation_reference: "QUOTE-CARRIER-RELEASED",
    note: "Actual release confirmed before replacement",
  });
  await page.reload();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Quotes", exact: true })
    .click();
  await page
    .getByRole("button", { name: `Open quote ${reference}`, exact: true })
    .click();
  await details
    .getByRole("button", { name: "Record replacement procurement" })
    .click();
  await dialog.getByLabel("Quoted carrier").selectOption("ca1");
  await dialog.getByLabel("Actual carrier offer (USD)").fill("1300.01");
  await dialog
    .getByLabel("Replacement procurement reason")
    .fill("Original carrier released; retain customer sell price");
  await proof("REPLACEMENT-CARRIER-RATE");
  await dialog.getByRole("button", { name: "Save quote action" }).click();
  await expect(dialog).not.toBeVisible();
  await expect(details).toContainText("Procurement 2: Summit Transport");
  await page.setViewportSize({ width: 390, height: 844 });
  await details.getByRole("button", { name: "Reconfirm carrier rate" }).click();
  await proof("SAME-RATE-EXTENSION");
  expect(await dialog.evaluate((e) => e.scrollWidth <= e.clientWidth)).toBe(
    true,
  );
  await dialog.getByRole("button", { name: "Save quote action" }).click();
  await expect(dialog).not.toBeVisible();
  await page.setViewportSize({ width: 1280, height: 1100 });
  w = await snapshot();
  quote = w.quotes.find((q) => q.reference === reference);
  expect(quote.procurements).toHaveLength(2);
  expect(quote.revisions[0].offers[0].amount_cents).toBe(125001);
  expect(w.loads.find((l) => l.id === load_id)).toMatchObject({
    status: "planned",
    buy_cents: 130001,
    sell_cents: 150001,
  });
  await details.getByRole("button", { name: "Open booked shipment" }).click();
  await dialog.getByRole("button", { name: "Tender", exact: true }).click();
  await expect(
    dialog
      .getByLabel("Carrier", { exact: true })
      .locator('option[value="ca4"]'),
  ).toHaveCount(0);
  await dialog.getByLabel("Carrier", { exact: true }).selectOption("ca1");
  await dialog
    .getByRole("button", { name: "Confirm tender", exact: true })
    .click();
  await expect(dialog).not.toBeVisible();
  if (!testInfo.retry) {
    await expect(page.locator(".toast")).toHaveCount(0, { timeout: 7000 });
    await details.screenshot({ path: "../docs/screenshots/quotes.png" });
  }
  const viewer = await browser.newContext();
  try {
    const session = await viewer.request.post(`${origin}/api/session`, {
      headers: { Origin: origin },
      data: { token: "freight-test-viewer-credential-at-least-32-chars" },
    });
    expect(session.ok()).toBe(true);
    const p = await viewer.newPage();
    await p.goto(origin);
    await p
      .getByRole("navigation")
      .getByRole("button", { name: "Quotes", exact: true })
      .click();
    await p
      .getByRole("button", { name: `Open quote ${reference}`, exact: true })
      .click();
    await expect(
      p.getByRole("button", { name: "Create quote", exact: true }),
    ).toHaveCount(0);
    await expect(
      p.getByRole("link", { name: "Download customer quote", exact: true }),
    ).toBeVisible();
    const denied = await viewer.request.post(
      `${origin}/api/freight/quotes/${quote.id}/reprocure`,
      {
        headers: { Origin: origin, "Idempotency-Key": randomUUID() },
        data: {},
      },
    );
    expect(denied.status()).toBe(403);
  } finally {
    await viewer.close();
  }
  expect(errors).toEqual([]);
});

test("cancellation fees retain carrier release, separate billing, paid reversal and refund without delivery", async ({
  page,
}, testInfo) => {
  test.setTimeout(90000);
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned cancellation workspace",
  );
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  const origin = new URL(page.url()).origin;
  const post = async (path, data) => {
    const response = await page.request.post(`/api/freight/${path}`, {
      headers: { Origin: origin, "Idempotency-Key": randomUUID() },
      data,
    });
    expect(response.ok(), await response.text()).toBe(true);
    return (await response.json()).data;
  };
  const workspace = async () => {
    const response = await page.request.get("/api/freight/workspace");
    expect(response.ok()).toBe(true);
    return response.json();
  };
  const stamp = Date.now(),
    reference = `E2E-CANCEL-${stamp}`,
    date = new Date(Date.now() + 86400000).toISOString().slice(0, 10);
  const created = await post("loads", {
    reference,
    customer_id: "c1",
    mode: "FTL",
    equipment: "Dry van",
    commodity: "Cancelled consumer goods",
    weight_lbs: 1000,
    miles: 173,
    buy_cents: 142000,
    sell_cents: 178000,
    origin: {
      facility: "Portland dock",
      city: "Portland",
      state: "OR",
      date,
      address_lines: ["15 Dock Street"],
      postal_code: "97201",
      country: "US",
    },
    destination: {
      facility: "Seattle receiving",
      city: "Seattle",
      state: "WA",
      date,
      address_lines: ["25 Receiving Ave"],
      postal_code: "98101",
      country: "US",
    },
  });
  const current = async () =>
    (await workspace()).loads.find((v) => v.id === created.id);
  await post(`loads/${created.id}/commands`, {
    action: "tender",
    expected_version: (await current()).version,
    carrier_id: "ca4",
  });
  await post(`loads/${created.id}/commands`, {
    action: "accept",
    expected_version: (await current()).version,
  });
  const release = await post(`loads/${created.id}/release-requests`, {
    expected_load_version: (await current()).version,
    disposition: "cancel",
    reason: "Customer cancelled before loading",
  });
  await post(`release-requests/${release.id}/confirm`, {
    expected_load_version: (await current()).version,
    carrier_contact: "Carrier dispatch",
    confirmation_reference: `RELEASE-${stamp}`,
    note: "Carrier confirmed release before pickup; fee reviewed separately",
  });
  const proof = Buffer.from("%PDF-1.7\nowned cancellation agreement\n%%EOF");
  const upload = await page.request.post(
    `/api/freight/loads/${created.id}/documents?kind=rate_confirmation&filename=cancellation.pdf`,
    {
      headers: {
        Origin: origin,
        "Idempotency-Key": randomUUID(),
        "Content-Type": "application/pdf",
      },
      data: proof,
    },
  );
  expect(upload.ok(), await upload.text()).toBe(true);
  const document = (await upload.json()).data;
  await page.reload();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /Load board/ })
    .click();
  await page.getByRole("button", { name: reference, exact: true }).click();
  const panel = page.getByRole("dialog"),
    section = panel.getByRole("region", { name: "Cancellation fees" }),
    form = section.getByRole("form", { name: "Cancellation fee form" });
  await expect(section).toContainText("This shipment remains cancelled");
  async function propose(side, amount, ref) {
    await section
      .getByRole("button", { name: "Propose cancellation fee", exact: true })
      .click();
    await form
      .getByLabel("Cancellation fee side", { exact: true })
      .selectOption(side);
    await form
      .getByLabel("Cancellation fee amount (USD)", { exact: true })
      .fill(amount);
    await form
      .getByLabel("Cancellation fee description", { exact: true })
      .fill(
        side === "buy"
          ? "Agreed truck ordered not used"
          : "Customer cancellation agreement",
      );
    await form
      .getByLabel("Cancellation agreement reference", { exact: true })
      .fill(ref);
    await form
      .getByLabel("Cancellation evidence file", { exact: true })
      .selectOption(document.id);
    await form.getByRole("button", { name: "Save cancellation fee" }).click();
    await expect(form).not.toBeVisible();
  }
  async function decision(kind, number) {
    await form
      .getByLabel("Cancellation counterparty contact")
      .fill("Verified counterparty accounts");
    await form
      .getByLabel("Cancellation confirmation reference")
      .fill(`CONFIRMED-${stamp}`);
    await form
      .getByLabel("Cancellation decision note")
      .fill("Reviewed actual agreement and invoice");
    if (kind === "approve")
      await form.getByLabel(/cancellation invoice number/).fill(number);
    await form.getByRole("button", { name: "Save cancellation fee" }).click();
    await expect(form).not.toBeVisible();
  }
  await propose("buy", "250.01", `BUY-${stamp}`);
  await section
    .getByRole("button", { name: "Approve cancellation fee", exact: true })
    .click();
  await decision("approve", `TONU-${stamp}`);
  await propose("sell", "375.01", `SELL-${stamp}`);
  await section
    .getByRole("button", { name: "Approve cancellation fee", exact: true })
    .click();
  await decision("approve", `CANCEL-${stamp}`);
  let w = await workspace();
  const buy = w.cancellation_fees.find(
      (v) => v.load_id === created.id && v.side === "buy",
    ),
    sell = w.cancellation_fees.find(
      (v) => v.load_id === created.id && v.side === "sell",
    );
  expect(w.loads.find((v) => v.id === created.id)).toMatchObject({
    status: "cancelled",
    pod_reference: null,
    carrier_id: null,
    commercial: {
      total_buy_cents: 25001,
      total_sell_cents: 37501,
      margin_cents: 12500,
    },
  });
  expect(w.invoices.some((v) => v.load_id === created.id)).toBe(false);
  const download = page.waitForEvent("download");
  await section
    .getByRole("link", { name: "Download cancellation invoice" })
    .click();
  const downloaded = await download;
  const statement = (await readFile(await downloaded.path())).toString();
  expect(statement).toContain("CANCELLATION INVOICE");
  expect(statement).toContain("Shipment cancelled");
  expect(statement).not.toContain("Freight services");
  expect(statement).not.toContain(`BUY-${stamp}`);
  async function cash(direction, target, amount, bank) {
    w = await workspace();
    const item =
      direction === "receipt"
        ? w.bills.find((v) => v.id === target)
        : w.settlements.find((v) => v.id === target);
    return post("cash", {
      direction,
      account: "Owned cancellation bank fixture",
      external_reference: bank,
      amount_cents: amount,
      currency: "USD",
      cleared_on: new Date().toISOString().slice(0, 10),
      note: "Actual synthetic cleared bank evidence",
      allocations: [
        {
          target_id: target,
          expected_version: item.version,
          amount_cents: amount,
        },
      ],
    });
  }
  await cash("payout", buy.target_id, 25001, `PAYOUT-${stamp}`);
  await cash("receipt", sell.target_id, 37501, `RECEIPT-${stamp}`);
  await panel
    .getByRole("button", { name: "Close dialog", exact: true })
    .click();
  await page.reload();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /Load board/ })
    .click();
  await page.getByRole("button", { name: reference, exact: true }).click();
  await expect(section).toContainText("Balance $0.00");
  const buyCard = section
    .locator("article")
    .filter({ hasText: `TONU-${stamp}` });
  await buyCard
    .getByRole("button", { name: "Reverse cancellation fee" })
    .click();
  await decision("reverse");
  await expect(buyCard).toContainText("Balance -$250.01");
  await cash("carrier_refund", buy.target_id, 25001, `REFUND-${stamp}`);
  await panel
    .getByRole("button", { name: "Close dialog", exact: true })
    .click();
  await page.reload();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /Load board/ })
    .click();
  await page.getByRole("button", { name: reference, exact: true }).click();
  await panel
    .getByRole("button", { name: "Close dialog", exact: true })
    .click();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Freight audit", exact: true })
    .click();
  await page
    .getByRole("tab", { name: "Cancellation fees", exact: true })
    .click();
  const queue = page.getByRole("region", {
    name: "Cancellation finance queue",
  });
  await expect(queue).toContainText(`TONU-${stamp}`);
  await queue
    .getByRole("row")
    .filter({ hasText: `TONU-${stamp}` })
    .getByRole("button", { name: "Open cancelled shipment" })
    .click();
  await page.setViewportSize({ width: 390, height: 844 });
  await section
    .getByRole("button", { name: "Propose cancellation fee", exact: true })
    .click();
  await form
    .getByLabel("Cancellation fee side", { exact: true })
    .selectOption("sell");
  await form
    .getByLabel("Cancellation fee amount (USD)", { exact: true })
    .fill("1.01");
  const rect = await form.boundingBox();
  expect(rect.width).toBeLessThanOrEqual(390);
  await form.getByRole("button", { name: "Close fee form" }).click();
  w = await workspace();
  expect(w.settlements.find((v) => v.id === buy.target_id).balance_cents).toBe(
    0,
  );
  expect(w.loads.find((v) => v.id === created.id).status).toBe("cancelled");
  if (!testInfo.retry) {
    await page.setViewportSize({ width: 1280, height: 1100 });
    await section.screenshot({
      path: "../docs/screenshots/cancellation-fees.png",
    });
  }
  expect(errors).toEqual([]);
});

test("customer supplements preserve paid original billing and reconcile their own credit and receipt", async ({
  page,
}, testInfo) => {
  test.setTimeout(90000);
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned synthetic billing workspace",
  );
  const origin = new URL(page.url()).origin;
  const post = async (path, data) => {
    const response = await page.request.post(`/api/freight/${path}`, {
      headers: { Origin: origin, "Idempotency-Key": randomUUID() },
      data,
    });
    expect(response.ok(), await response.text()).toBe(true);
    return (await response.json()).data;
  };
  const snapshot = async () =>
    (await page.request.get("/api/freight/workspace")).json();
  const today = new Date().toISOString().slice(0, 10);
  const suffix = Date.now(),
    primaryNumber = `PRIMARY-${suffix}`,
    supplementNumber = `SUP-${suffix}`;
  const customer = await post("customers", {
    name: "Supplement billing customer",
    contact: "ap@example.test",
    billing_address: "25 Receiver Way, Seattle WA 98101",
  });
  let load = await post("loads", {
    reference: `E2E-SUP-${suffix}`,
    customer_id: customer.id,
    mode: "FTL",
    equipment: "Dry van",
    commodity: "Supplement fixture",
    weight_lbs: 1000,
    miles: 173,
    buy_cents: 118000,
    sell_cents: 149000,
    origin: {
      facility: "Origin",
      city: "Portland",
      state: "OR",
      date: today,
      address_lines: ["15 Dock Street"],
      postal_code: "97201",
      country: "US",
    },
    destination: {
      facility: "Receiver",
      city: "Seattle",
      state: "WA",
      date: today,
      address_lines: ["25 Receiver Way"],
      postal_code: "98101",
      country: "US",
    },
  });
  load = await post(`loads/${load.id}/commands`, {
    action: "tender",
    expected_version: load.version,
    carrier_id: "ca4",
  });
  load = await post(`loads/${load.id}/commands`, {
    action: "accept",
    expected_version: load.version,
  });
  let w = await snapshot();
  const tender = w.tenders.find((v) => v.id === load.tender_id);
  await post("events", {
    event_id: randomUUID(),
    event_type: "edi.shipment_status.received",
    occurred_at: new Date().toISOString(),
    payload: {
      shipment_id: tender.shipment_reference,
      partner_id: "brex",
      carrier_scac: "BREX",
      latest_status: "D1",
      events: [{ status_code: "AF" }, { status_code: "D1" }],
    },
  });
  w = await snapshot();
  load = await post(`loads/${load.id}/commands`, {
    action: "attach_pod",
    expected_version: w.loads.find((v) => v.id === load.id).version,
    pod_reference: "External signed evidence for supplemental billing",
  });
  await post(`loads/${load.id}/pod/review`, {
    expected_version: load.version,
    decision: "accepted",
    note: "Verified signed delivery evidence",
  });
  const primaryDraft = await post("bills", {
    load_id: load.id,
    number: primaryNumber,
    due_date: today,
  });
  const primary = await post(`bills/${primaryDraft.id}/issue`, {
    expected_version: primaryDraft.version,
  });
  await post("cash", {
    direction: "receipt",
    currency: "USD",
    account: "Operating",
    external_reference: `PRIMARY-PAID-${suffix}`,
    amount_cents: 149000,
    cleared_on: today,
    note: "Verified original paid bank deposit",
    allocations: [
      {
        target_id: primary.id,
        expected_version: primary.version,
        amount_cents: 149000,
      },
    ],
  });
  const uploaded = await page.request.post(
    `/api/freight/loads/${load.id}/documents?kind=other&filename=customer-agreement.pdf`,
    {
      headers: {
        Origin: origin,
        "Idempotency-Key": randomUUID(),
        "Content-Type": "application/pdf",
      },
      data: Buffer.from("%PDF-1.4\ncustomer-approved extra unloading\n%%EOF\n"),
    },
  );
  expect(uploaded.ok()).toBe(true);
  const document = (await uploaded.json()).data;
  await page.reload();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Freight audit", exact: true })
    .click();
  await page
    .getByRole("tab", { name: "Customer billing", exact: true })
    .click();
  const originalRow = page
    .getByRole("row")
    .filter({ has: page.getByText(primaryNumber, { exact: true }) });
  await expect(originalRow.getByText("Paid", { exact: true })).toBeVisible();
  await originalRow
    .getByRole("button", { name: "Draft supplement", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Supplement invoice number").fill(supplementNumber);
  await dialog.getByLabel("Supplement amount (USD)").fill("125.01");
  await dialog
    .getByLabel("Customer charge description")
    .fill("Additional unloading agreed after original billing");
  await dialog
    .getByLabel("Customer agreement contact")
    .fill("Customer AP representative");
  await dialog
    .getByLabel("Customer confirmation reference")
    .fill(`CONF-${suffix}`);
  await dialog
    .getByLabel("Customer agreement reference")
    .fill(`UNLOADING-${suffix}`);
  await dialog
    .getByLabel("Stored agreement evidence (optional)")
    .selectOption(document.id);
  await dialog
    .getByRole("button", { name: "Create supplemental draft", exact: true })
    .click();
  await expect(dialog).not.toBeVisible();
  const row = page.getByRole("row").filter({
    has: page.getByRole("link", {
      name: `Download invoice ${supplementNumber}`,
      exact: true,
    }),
  });
  await expect(row.getByText("$125.01", { exact: true })).toBeVisible();
  await row.getByRole("button", { name: "Issue invoice", exact: true }).click();
  await dialog
    .getByRole("button", { name: "Confirm issue", exact: true })
    .click();
  await expect(row.getByText("Issued", { exact: true })).toBeVisible();
  await row.getByRole("button", { name: "Credit note", exact: true }).click();
  await dialog.getByLabel("Credit amount (USD)").fill("25.01");
  await dialog
    .getByLabel("Finance reason")
    .fill("Customer verified service correction");
  await dialog
    .getByRole("button", { name: "Post credit note", exact: true })
    .click();
  await expect(dialog).not.toBeVisible();
  await row
    .getByRole("button", { name: "Record receipt", exact: true })
    .click();
  await dialog.getByLabel("Bank amount (USD)").fill("100.00");
  await dialog
    .getByLabel(`Allocation amount ${supplementNumber}`)
    .fill("100.00");
  await dialog.getByLabel("Cleared bank reference").fill(`SUP-PAID-${suffix}`);
  await dialog
    .getByLabel("Reconciliation note")
    .fill("Matched cleared supplemental customer deposit");
  await dialog
    .getByRole("button", { name: "Record reconciliation", exact: true })
    .click();
  await expect(dialog).not.toBeVisible();
  await expect(row.getByText("Paid", { exact: true })).toBeVisible();
  await expect(originalRow.getByText("Paid", { exact: true })).toBeVisible();
  await row.getByText("Customer agreement", { exact: true }).click();
  await expect(
    row.getByText(`Customer AP representative · CONF-${suffix}`, {
      exact: true,
    }),
  ).toBeVisible();
  const statement = await page.request.get(
    `/api/freight/bills/${(await snapshot()).bills.find((v) => v.number === supplementNumber).id}/statement`,
  );
  const output = await statement.text();
  expect(output).toContain("SUPPLEMENTAL CUSTOMER INVOICE");
  expect(output).toContain(`Supplement to invoice ${primaryNumber}`);
  expect(output).not.toContain("1490.00");
  expect(output).not.toContain("1180.00");
  expect(
    await row.evaluate((el) => {
      const table = el.closest(".table-scroll");
      return table.scrollWidth - table.clientWidth;
    }),
  ).toBeLessThanOrEqual(1);
  if (process.env.FREIGHT_E2E_SCREENSHOTS === "true")
    await page.screenshot({
      path: "../docs/screenshots/customer-supplements.png",
      fullPage: true,
    });
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(
    row.getByRole("link", {
      name: `Download invoice ${supplementNumber}`,
      exact: true,
    }),
  ).toBeVisible();
  w = await snapshot();
  expect(w.bills.find((v) => v.id === primary.id).snapshot).toEqual(
    primary.snapshot,
  );
  expect(w.bills.find((v) => v.id === primary.id).totals.received_cents).toBe(
    149000,
  );
  expect(
    w.loads.find((v) => v.id === load.id).commercial.total_sell_cents,
  ).toBe(159000);
  expect(
    w.cash_entries.filter((v) =>
      v.allocations.some((a) => a.load_id === load.id),
    ),
  ).toHaveLength(2);
  await page.setViewportSize({ width: 1280, height: 900 });
  await originalRow
    .getByRole("button", { name: "Draft supplement", exact: true })
    .click();
  const wrongNumber = `WRONG-${suffix}`,
    correctedNumber = `CORRECTED-${suffix}`;
  await dialog.getByLabel("Supplement invoice number").fill(wrongNumber);
  await dialog.getByLabel("Supplement amount (USD)").fill("49.99");
  await dialog
    .getByLabel("Customer charge description")
    .fill("Second customer-agreed service");
  await dialog
    .getByLabel("Customer agreement contact")
    .fill("Customer AP representative");
  await dialog
    .getByLabel("Customer confirmation reference")
    .fill(`SECOND-CONF-${suffix}`);
  await dialog
    .getByLabel("Customer agreement reference")
    .fill(`SECOND-AGREEMENT-${suffix}`);
  await dialog
    .getByRole("button", { name: "Create supplemental draft", exact: true })
    .click();
  await expect(dialog).not.toBeVisible();
  const wrongRow = page
    .getByRole("row")
    .filter({ has: page.getByText(wrongNumber, { exact: true }) });
  await wrongRow
    .getByRole("button", { name: "Void invoice", exact: true })
    .click();
  await dialog
    .getByLabel("Finance reason")
    .fill("Incorrect draft amount; customer confirmed 50.01");
  await dialog
    .getByRole("button", { name: "Confirm void", exact: true })
    .click();
  await expect(dialog).not.toBeVisible();
  await wrongRow
    .getByRole("button", { name: "Correct void supplement", exact: true })
    .click();
  await expect(
    dialog.getByLabel("Customer agreement reference"),
  ).toHaveAttribute("readonly", "");
  await expect(dialog.getByLabel("Customer agreement reference")).toHaveValue(
    `SECOND-AGREEMENT-${suffix}`,
  );
  await dialog.getByLabel("Supplement invoice number").fill(correctedNumber);
  await dialog.getByLabel("Supplement amount (USD)").fill("50.01");
  await dialog
    .getByLabel("Customer agreement contact")
    .fill("Customer AP representative");
  await dialog
    .getByLabel("Customer confirmation reference")
    .fill(`SECOND-CORRECTED-${suffix}`);
  await dialog
    .getByRole("button", { name: "Create supplemental draft", exact: true })
    .click();
  await expect(dialog).not.toBeVisible();
  await expect(
    wrongRow.getByRole("button", {
      name: "Correct void supplement",
      exact: true,
    }),
  ).toHaveCount(0);
  w = await snapshot();
  const wrong = w.bills.find((v) => v.number === wrongNumber),
    corrected = w.bills.find((v) => v.number === correctedNumber);
  expect(wrong.status).toBe("void");
  expect(wrong.supplement.corrected_by_id).toBe(corrected.id);
  expect(corrected.supplement.corrects_id).toBe(wrong.id);
  expect(corrected.snapshot.amount_cents).toBe(5001);
  expect(
    w.loads.find((v) => v.id === load.id).commercial.total_sell_cents,
  ).toBe(159000);
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /Load board/ })
    .click();
  await page.getByLabel("Search loads").fill(load.reference);
  const exportReady = page.waitForEvent("download");
  await page.getByRole("button", { name: "Export", exact: true }).click();
  const csvPath = testInfo.outputPath("supplement-load.csv");
  await (await exportReady).saveAs(csvPath);
  const csv = await readFile(csvPath, "utf8");
  expect(csv).toContain('"Net sell additions and credits","Currency"');
  expect(csv).toContain(
    '"1590.00","410.00","1180.00","0.00","1490.00","100.00"',
  );
});

test("staff sessions enforce live membership and organization isolation through the web gateway", async ({
  browser,
  page,
}) => {
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned Clerk protocol fixture",
  );
  const origin = clerkOrigin;
  const context = await browser.newContext({ baseURL: origin });
  try {
    const request = context.request;
    const fixture = async (path, data = {}) => {
      const result = await request.post(`${fixtureOrigin}/__identity/${path}`, {
        headers: { "x-fixture-key": "freight-fixture-only" },
        data,
      });
      expect(result.ok()).toBe(true);
      return result.json();
    };
    await fixture("control", {
      status: "active",
      role: "org:operations",
      member: true,
      org: "org_staff",
      outage: false,
    });
    const legacy = (await page.context().cookies()).find(
      (v) => v.name === "freight_session",
    );
    await context.addCookies([
      { name: "freight_session", value: legacy.value, url: origin },
    ]);
    expect(
      (
        await request.get("/api/freight/workspace?organization_id=org_staff")
      ).status(),
    ).toBe(401);
    expect(
      (
        await request.post("/api/session", {
          headers: { Origin: origin },
          data: { token: credential },
        })
      ).status(),
    ).toBe(403);
    const { token } = await fixture("token");
    await context.addCookies([
      { name: "__session", value: token, url: origin, sameSite: "Strict" },
    ]);
    const headers = {
      Origin: origin,
      "X-Freight-Organization-Id": "org_staff",
      Authorization: `Bearer ${token}`,
      "Idempotency-Key": randomUUID(),
    };
    let result = await request.get(
      "/api/freight/workspace?organization_id=org_staff",
    );
    expect(result.ok(), await result.text()).toBe(true);
    let w = await result.json();
    expect(w.tenant_id).toBe("identity");
    expect(w.access.subject).toBe("clerk:user_staff");
    expect(w.access.role).toBe("operations");
    const customer = await request.post("/api/freight/customers", {
      headers,
      data: {
        name: `SSO customer ${Date.now()}`,
        contact: "staff-test@example.test",
      },
    });
    expect(customer.ok()).toBe(true);
    const customerId = (await customer.json()).data.id;
    expect(
      (
        await request.post("/api/freight/bills", {
          headers: { ...headers, "Idempotency-Key": randomUUID() },
          data: {},
        })
      ).status(),
    ).toBe(403);
    const today = new Date().toISOString().slice(0, 10);
    result = await request.post("/api/freight/loads", {
      headers: { ...headers, "Idempotency-Key": randomUUID() },
      data: {
        reference: `SSO-${Date.now()}`,
        customer_id: customerId,
        mode: "FTL",
        equipment: "Dry van",
        commodity: "Staff fixture",
        weight_lbs: 1000,
        miles: 100,
        buy_cents: 10000,
        sell_cents: 15000,
        origin: {
          facility: "Origin",
          city: "Portland",
          state: "OR",
          date: today,
          address_lines: ["15 Dock Street"],
          postal_code: "97201",
          country: "US",
        },
        destination: {
          facility: "Receiver",
          city: "Seattle",
          state: "WA",
          date: today,
          address_lines: ["25 Receiver Way"],
          postal_code: "98101",
          country: "US",
        },
      },
    });
    expect(result.ok(), await result.text()).toBe(true);
    const load = (await result.json()).data;
    const bytes = Buffer.from("%PDF-1.4\nowned SSO binary evidence\n%%EOF\n");
    result = await request.post(
      `/api/freight/loads/${load.id}/documents?kind=other&filename=staff-evidence.pdf`,
      {
        headers: {
          ...headers,
          "Content-Type": "application/pdf",
          "Idempotency-Key": randomUUID(),
        },
        data: bytes,
      },
    );
    expect(result.ok(), await result.text()).toBe(true);
    const document = (await result.json()).data;
    const download = `/api/freight/documents/${document.id}/content?organization_id=org_staff`;
    result = await request.get(download);
    expect(result.ok()).toBe(true);
    expect(await result.body()).toEqual(bytes);
    expect(result.headers()["x-content-sha256"]).toMatch(/^[a-f0-9]{64}$/);
    result = await request.get(
      "/api/freight/finance/journal.csv?organization_id=org_staff",
    );
    expect(result.ok()).toBe(true);
    expect(result.headers()["content-type"]).toContain("text/csv");
    await fixture("control", { role: "org:viewer" });
    expect(
      (
        await request.post("/api/freight/customers", {
          headers,
          data: { name: "Downgraded replay", contact: "test@example.test" },
        })
      ).status(),
    ).toBe(403);
    result = await request.get(
      "/api/freight/workspace?organization_id=org_staff",
    );
    w = await result.json();
    expect(w.access.role).toBe("viewer");
    expect(
      w.events.find(
        (v) => v.kind === "customer.create" || v.kind === "customer.created",
      )?.actor || w.events[0].actor,
    ).toBe("clerk:user_staff");
    await fixture("control", { role: "org:operations", org: "org_other" });
    const other = (await fixture("token")).token;
    await context.addCookies([
      { name: "__session", value: other, url: origin, sameSite: "Strict" },
    ]);
    expect((await request.get(download)).status()).toBe(403);
    result = await request.get(
      "/api/freight/workspace?organization_id=org_other",
    );
    expect(result.ok()).toBe(true);
    w = await result.json();
    expect(w.tenant_id).toBe("identity_other");
    expect(w.customers).toEqual([]);
    expect(
      (
        await request.get(
          `/api/freight/documents/${document.id}/content?organization_id=org_other`,
        )
      ).status(),
    ).toBe(404);
    expect(
      (
        await request.post("/api/freight/customers", {
          headers,
          data: {
            name: "Stale organization action",
            contact: "test@example.test",
          },
        })
      ).status(),
    ).toBe(403);
    await fixture("control", { org: "org_staff", member: false });
    await context.addCookies([
      { name: "__session", value: token, url: origin, sameSite: "Strict" },
    ]);
    expect(
      (
        await request.get("/api/freight/workspace?organization_id=org_staff")
      ).status(),
    ).toBe(403);
    await fixture("control", { member: true, status: "revoked" });
    expect(
      (
        await request.get("/api/freight/workspace?organization_id=org_staff")
      ).status(),
    ).toBe(401);
    await fixture("control", { status: "active", outage: true });
    expect(
      (
        await request.get("/api/freight/workspace?organization_id=org_staff")
      ).status(),
    ).toBe(503);
    expect((await request.get("/api/freight/workspace")).status()).toBe(
      401,
      "an explicit organization is required even when a valid cookie exists",
    );
    const signIn = await context.newPage();
    await signIn.route("https://**/*", (route) => route.abort());
    await signIn.clock.install();
    await signIn.goto(origin);
    await expect(signIn.locator("main")).toBeVisible();
    await signIn.clock.fastForward(15000);
    await expect(
      signIn.getByRole("heading", {
        name: "Staff sign-in unavailable",
        exact: true,
      }),
    ).toBeVisible();
    await expect(
      signIn.getByRole("button", { name: "Retry staff sign-in", exact: true }),
    ).toBeVisible();
    await expect(signIn.getByLabel("Workspace credential")).toHaveCount(0);
    await signIn.close();
  } finally {
    await context.close();
  }
});

test("workspace sign-in creates a protected session and logout revokes browser access", async ({
  page,
}) => {
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned authenticated web server",
  );
  const session = (await page.context().cookies()).find(
    (v) => v.name === "freight_session",
  );
  expect(session).toBeTruthy();
  expect(session.httpOnly).toBe(true);
  expect(session.sameSite).toBe("Strict");
  expect(session.value).not.toContain(credential);
  const origin = new URL(page.url()).origin;
  expect(
    (
      await page.request.delete("/api/session", { headers: { Origin: origin } })
    ).ok(),
  ).toBe(true);
  expect((await page.request.get("/api/freight/workspace")).status()).toBe(401);
  expect(
    (
      await page.request.post("/api/session", {
        headers: { Origin: origin },
        data: { token: "invalid-operator-credential-at-least-32-chars" },
      })
    ).status(),
  ).toBe(401);
  await page.reload();
  await expect(page.getByLabel("Workspace credential")).toBeVisible();
});

test("workspace roles keep read access useful while enforcing operations and finance boundaries", async ({
  page,
}) => {
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned scoped credentials",
  );
  const origin = new URL(page.url()).origin;
  const login = async (role) => {
    const response = await page.request.post("/api/session", {
      headers: { Origin: origin },
      data: { token: `freight-test-${role}-credential-at-least-32-chars` },
    });
    expect(response.ok()).toBe(true);
    await page.goto("/");
    await expect(page.getByLabel("Workspace access role")).toHaveText(
      `${role[0].toUpperCase()}${role.slice(1)} access`,
    );
  };
  await login("viewer");
  await page
    .getByLabel("Operations perspective")
    .selectOption("Carrier operations");
  await expect(
    page.getByRole("button", { name: "Create load", exact: true }),
  ).toBeDisabled();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /^Load board/ })
    .click();
  await page.getByLabel("Search loads").fill("FR-1052");
  await page.getByRole("button", { name: "FR-1052", exact: true }).click();
  await expect(page.getByRole("dialog")).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Edit plan", exact: true }),
  ).toBeDisabled();
  await page.getByLabel("Close dialog").click();
  const denied = await page.request.post("/api/freight/customers", {
    headers: {
      Origin: origin,
      "Idempotency-Key": randomUUID(),
      "x-freight-role": "admin",
    },
    data: { name: "Unauthorized customer", contact: "test@example.test" },
  });
  expect(denied.status()).toBe(403);
  await login("operations");
  await expect(
    page.getByRole("button", { name: "Create load", exact: true }),
  ).toBeEnabled();
  await page
    .getByRole("button", { name: "Freight audit", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "Add invoice", exact: true }),
  ).toBeDisabled();
  await expect(
    page.getByRole("button", { name: "Approve", exact: true }).first(),
  ).toBeDisabled();
  await login("finance");
  await expect(
    page.getByRole("button", { name: "Create load", exact: true }),
  ).toBeDisabled();
  await page
    .getByRole("button", { name: "Freight audit", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "Add invoice", exact: true }),
  ).toBeEnabled();
  await expect(
    page.getByRole("button", { name: "Approve", exact: true }).first(),
  ).toBeEnabled();
  await login("integration");
  await expect(
    page.getByRole("button", { name: "Create load", exact: true }),
  ).toBeDisabled();
  await page.getByRole("button", { name: "Integrations", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Inbound EDI documents" }),
  ).toBeVisible();
});

test("operator recovers a retained 214 from the inbound queue without losing carrier evidence", async ({
  page,
}) => {
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned EDI fixture and recovery tenant",
  );
  const origin = new URL(page.url()).origin;
  await page.request.delete("/api/session", { headers: { Origin: origin } });
  const login = await page.request.post("/api/session", {
    headers: { Origin: origin },
    data: { token: "freight-recovery-test-credential-at-least-32-chars" },
  });
  expect(login.ok()).toBe(true);
  async function post(path, data) {
    const response = await page.request.post(`/api/freight/${path}`, {
      headers: { Origin: origin, "Idempotency-Key": randomUUID() },
      data,
    });
    expect(response.ok(), await response.text()).toBe(true);
    return (await response.json()).data;
  }
  const today = new Date().toISOString().slice(0, 10);
  const ref = `REC-${Date.now()}`;
  const customer = await post("customers", {
    name: "Recovery test shipper",
    contact: "ops@example.test",
  });
  const carrier = await post("carriers", {
    name: "Recovery test carrier",
    scac: "TEST",
    mc_number: "MC-TEST",
    insurance_expires: new Date(Date.now() + 365 * 86400000)
      .toISOString()
      .slice(0, 10),
    equipment: "Dry van",
    partner_id: "recovery-carrier",
  });
  const load = await post("loads", {
    reference: ref,
    customer_id: customer.id,
    mode: "FTL",
    equipment: "Dry van",
    commodity: "Test goods",
    weight_lbs: 24000,
    miles: 180,
    buy_cents: 120000,
    sell_cents: 150000,
    origin: {
      facility: "Portland DC",
      city: "Portland",
      state: "OR",
      date: today,
      address_lines: ["15 Dock Street"],
      postal_code: "97201",
      country: "US",
    },
    destination: {
      facility: "Seattle DC",
      city: "Seattle",
      state: "WA",
      date: today,
      address_lines: ["25 Receiving Ave"],
      postal_code: "98101",
      country: "US",
    },
  });
  const tender = await post(`loads/${load.id}/commands`, {
    action: "tender",
    expected_version: 1,
    carrier_id: carrier.id,
  });
  const doc = `doc-${randomUUID()}`;
  const tenderWorkspace = await (
    await page.request.get("/api/freight/workspace")
  ).json();
  const wireReference = tenderWorkspace.tenders.find(
    (attempt) => attempt.load_id === load.id,
  ).shipment_reference;
  const fixture = await page.request.put(`${fixtureOrigin}/fixtures/${doc}`, {
    headers: { "x-fixture-key": "freight-fixture-only" },
    data: {
      document: {
        id: doc,
        partner_id: "recovery-carrier",
        transaction_set: "214",
        direction: "inbound",
        status: "accepted",
        processed_at: new Date().toISOString(),
      },
      parsed: {
        shipment_id: wireReference,
        carrier_scac: "TEST",
        events: [
          {
            status_code: "AF",
            city: "Portland",
            state: "OR",
            date: today,
            time: "1200",
          },
          {
            status_code: "D1",
            city: "Seattle",
            state: "WA",
            date: today,
            time: "1600",
          },
        ],
      },
    },
  });
  expect(fixture.ok()).toBe(true);
  const body = JSON.stringify({
    type: "document.recorded",
    tenant_id: "recovery",
    document_id: doc,
    partner_id: "recovery-carrier",
    reference: ref,
    transaction_set: "214",
    direction: "inbound",
    status: "accepted",
  });
  const timestamp = String(Math.floor(Date.now() / 1000)),
    delivery = randomUUID();
  const signature = createHmac(
    "sha256",
    "freight-recovery-callback-secret-at-least-32-chars",
  )
    .update(`${timestamp}.${delivery}.${body}`)
    .digest("hex");
  const received = await page.request.post(
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
  expect(received.ok()).toBe(true);
  async function workspace() {
    const response = await page.request.get("/api/freight/workspace");
    expect(response.ok()).toBe(true);
    return response.json();
  }
  await expect
    .poll(async () => (await workspace()).inbox[0]?.status, { timeout: 10000 })
    .toBe("retry");
  await page.reload();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Integrations", exact: true })
    .click();
  await page.getByLabel("Filter inbound documents").selectOption("attention");
  await page.getByLabel("Search inbound documents").fill(doc);
  const row = page.getByRole("row").filter({ hasText: doc });
  await expect(row.getByText("Retry", { exact: true }).first()).toBeVisible();
  await page
    .getByRole("button", {
      name: `Inspect inbound document ${doc}`,
      exact: true,
    })
    .click();
  const dialog = page.getByRole("dialog");
  await expect(
    dialog.getByRole("heading", {
      name: "Original authenticated notification",
    }),
  ).toBeVisible();
  await expect(
    dialog.getByText("Cannot apply 214 status AF", { exact: false }),
  ).toHaveCount(2);
  await dialog.getByRole("button", { name: "Close dialog" }).click();
  await post(`loads/${load.id}/commands`, {
    action: "accept",
    expected_version: tender.version,
  });
  await page
    .getByRole("button", { name: `Retry inbound document ${doc}`, exact: true })
    .click();
  await expect
    .poll(async () => (await workspace()).inbox[0]?.status, { timeout: 10000 })
    .toBe("applied");
  await page.getByRole("button", { name: "Refresh", exact: true }).click();
  await page.getByLabel("Filter inbound documents").selectOption("all");
  await expect(row.getByText("Applied", { exact: true })).toBeVisible();
  await page
    .getByRole("button", {
      name: `Inspect inbound document ${doc}`,
      exact: true,
    })
    .click();
  await expect(
    dialog.getByRole("heading", { name: "Processing history" }),
  ).toBeVisible();
  await expect(
    dialog.getByRole("cell", { name: "Failed", exact: true }),
  ).toBeVisible();
  await expect(
    dialog.getByRole("cell", { name: "Applied", exact: true }),
  ).toBeVisible();
  const done = await workspace();
  expect(done.loads[0].status).toBe("delivered");
  expect(done.loads[0].pod_reference).toBeNull();
  expect(done.inbox[0].history.length).toBe(2);
});

test("operator withdraws and releases carrier tenders without accepting stale replies or losing conflicts", async ({
  page,
}) => {
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned synthetic tender workspace",
  );
  const origin = new URL(page.url()).origin;
  const post = async (path, body) => {
    const response = await page.request.post(`/api/freight/${path}`, {
      headers: { Origin: origin, "Idempotency-Key": randomUUID() },
      data: body,
    });
    expect(response.ok(), await response.text()).toBe(true);
    return (await response.json()).data;
  };
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /^Load board/ })
    .click();
  await page.getByLabel("Search loads").fill("FR-1053");
  const dialog = page.getByRole("dialog");
  const open = async () =>
    page.getByRole("button", { name: "FR-1053", exact: true }).click();
  const offer = async () => {
    await open();
    await dialog.getByRole("button", { name: "Tender", exact: true }).click();
    await dialog.getByLabel("Carrier", { exact: true }).selectOption("ca4");
    await dialog
      .getByRole("button", { name: "Confirm tender", exact: true })
      .click();
    await expect(dialog).not.toBeVisible();
  };
  await offer();
  await open();
  await dialog
    .getByRole("button", { name: "Withdraw unsent tender", exact: true })
    .click();
  await dialog
    .getByRole("button", { name: "Confirm withdrawal", exact: true })
    .click();
  await expect(dialog.locator(".detail-badges .badge")).toHaveText("Planned");
  await dialog.getByLabel("Close dialog").click();
  await offer();
  await open();
  await dialog.getByRole("button", { name: "Accept", exact: true }).click();
  await dialog
    .getByRole("button", { name: "Confirm accept", exact: true })
    .click();
  await open();
  let workspace = await (
    await page.request.get("/api/freight/workspace")
  ).json();
  const original = workspace.tenders
    .filter((attempt) => attempt.load_id === "load_8")
    .at(-1);
  await dialog
    .getByRole("button", { name: "Request carrier release", exact: true })
    .click();
  await dialog
    .getByLabel("Release reason")
    .fill("Carrier agreed to release the offer before pickup for replanning.");
  await dialog
    .getByRole("button", { name: "Create release request", exact: true })
    .click();
  await expect(
    dialog.getByText(
      /Carrier release pending · dispatch and reassignment on hold/,
    ),
  ).toBeVisible();
  await expect(
    dialog.getByRole("button", { name: "Dispatch", exact: true }),
  ).toBeDisabled();
  await dialog
    .getByRole("button", { name: "Confirm carrier released", exact: true })
    .click();
  await dialog.getByLabel("Carrier contact").fill("Named carrier dispatcher");
  await dialog
    .getByLabel("Carrier confirmation reference")
    .fill("CARRIER-RELEASE-TEST");
  await dialog
    .getByLabel("Carrier coordination note")
    .fill(
      "Carrier confirmed the exact shipment reference is released and will not be picked up.",
    );
  await dialog
    .getByRole("button", { name: "Record release confirmation", exact: true })
    .click();
  await expect(dialog.locator(".detail-badges .badge")).toHaveText("Planned");
  await dialog.getByLabel("Close dialog").click();
  await offer();
  workspace = await (await page.request.get("/api/freight/workspace")).json();
  const current = workspace.tenders
    .filter((attempt) => attempt.load_id === "load_8")
    .at(-1);
  expect(current.shipment_reference).not.toBe(original.shipment_reference);
  const ignored = await post("events", {
    event_id: `old-990-${randomUUID()}`,
    event_type: "edi.load_tender_response.received",
    occurred_at: new Date().toISOString(),
    payload: {
      shipment_id: original.shipment_reference,
      partner_id: "brex",
      accepted: true,
    },
  });
  expect(ignored.ignored).toBe(true);
  const held = await post("events", {
    event_id: `old-pickup-${randomUUID()}`,
    event_type: "edi.shipment_status.received",
    occurred_at: new Date().toISOString(),
    payload: {
      shipment_id: original.shipment_reference,
      partner_id: "brex",
      latest_status: "AF",
      events: [{ status_code: "AF" }],
    },
  });
  expect(held.held).toBe(true);
  await page.reload();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /^Load board/ })
    .click();
  await page.getByLabel("Search loads").fill("FR-1053");
  await open();
  await expect(dialog.locator(".detail-badges .badge")).toHaveText("Tendered");
  await expect(
    dialog.getByText("Carrier custody or invoice conflict", { exact: true }),
  ).toBeVisible();
  await dialog
    .getByRole("button", { name: "Resolve carrier conflict", exact: true })
    .click();
  await dialog.getByLabel("Carrier contact").fill("Carrier supervisor");
  await dialog
    .getByLabel("Carrier confirmation reference")
    .fill("CARRIER-SCAN-CORRECTION");
  await dialog
    .getByLabel("Carrier coordination note")
    .fill(
      "Carrier confirmed the old pickup scan was erroneous; the replacement assignment remains correct.",
    );
  await dialog
    .getByRole("button", { name: "Record conflict resolution", exact: true })
    .click();
  await expect(
    dialog.getByText("Carrier custody or invoice conflict", { exact: true }),
  ).toHaveCount(0);
  const panel = dialog.getByLabel("Tender history and carrier release");
  await expect(
    panel.getByText("Attempt 2 · Blue Ridge Express · Released", {
      exact: true,
    }),
  ).toBeVisible();
  if (process.env.FREIGHT_E2E_SCREENSHOTS === "true") {
    await dialog.evaluate((element) => {
      const history = element.querySelector(".tender-history");
      if (history)
        element.scrollTop +=
          history.getBoundingClientRect().top -
          element.getBoundingClientRect().top -
          24;
    });
    await page.screenshot({
      path: "../docs/screenshots/carrier-release.png",
      fullPage: true,
    });
  }
  await page.setViewportSize({ width: 390, height: 844 });
  expect(
    await dialog.evaluate(
      (element) => element.scrollWidth <= element.clientWidth,
    ),
  ).toBe(true);
});

test("operator completes planning, tender, dispatch, delivery, audit, and settlement", async ({
  page,
}) => {
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/");
  await expect(
    page.getByRole("heading", { name: "Overview", exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Create load", exact: true }).click();
  const dialog = page.getByRole("dialog");
  const reference = `E2E-${Date.now()}`;
  await dialog.getByLabel("Load reference").fill(reference);
  await dialog.getByLabel("Customer", { exact: true }).selectOption("c3");
  await dialog.getByLabel("Equipment", { exact: true }).selectOption("Flatbed");
  await dialog.getByLabel("Commodity").fill("Fabricated steel");
  await dialog
    .getByLabel("Facility", { exact: true })
    .nth(0)
    .fill("Denver plant");
  await dialog.getByLabel("City", { exact: true }).nth(0).fill("Denver");
  await dialog.getByLabel("State", { exact: true }).nth(0).fill("CO");
  await dialog
    .getByLabel("Pickup street address", { exact: true })
    .fill("15 Fabrication Way");
  await dialog.getByLabel("Pickup postal code", { exact: true }).fill("80201");
  await dialog
    .getByLabel("Facility", { exact: true })
    .nth(1)
    .fill("Phoenix receiver");
  await dialog.getByLabel("City", { exact: true }).nth(1).fill("Phoenix");
  await dialog.getByLabel("State", { exact: true }).nth(1).fill("AZ");
  await dialog
    .getByLabel("Delivery street address", { exact: true })
    .fill("25 Receiving Ave");
  await dialog
    .getByLabel("Delivery postal code", { exact: true })
    .fill("85001");
  await dialog.getByLabel("Buy rate", { exact: true }).fill("2000.01");
  await dialog.getByLabel("Sell rate", { exact: true }).fill("2500.25");
  await dialog.getByLabel("Distance (miles)").fill("860");
  await dialog
    .getByRole("button", { name: "Create load", exact: true })
    .click();
  await expect(dialog).not.toBeVisible();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /Load board/ })
    .click();
  await page.getByRole("button", { name: reference, exact: true }).click();
  await dialog.getByRole("button", { name: "Edit plan", exact: true }).click();
  await expect(dialog.getByLabel("Buy rate", { exact: true })).toHaveValue(
    "2000.01",
  );
  await dialog.getByLabel("Commodity").fill("Fabricated steel, secured");
  await dialog
    .getByRole("button", { name: "Save changes", exact: true })
    .click();
  await expect(dialog).not.toBeVisible();
  await page.getByRole("button", { name: reference, exact: true }).click();
  await dialog.getByRole("button", { name: "Tender", exact: true }).click();
  await dialog.getByLabel("Carrier", { exact: true }).selectOption("ca3");
  await dialog
    .getByRole("button", { name: "Confirm tender", exact: true })
    .click();
  await expect(dialog).not.toBeVisible();
  async function open() {
    await page.getByRole("button", { name: reference, exact: true }).click();
  }
  async function action(name, confirm, fill) {
    await open();
    await dialog.getByRole("button", { name, exact: true }).click();
    if (fill) await fill();
    await dialog.getByRole("button", { name: confirm, exact: true }).click();
    await expect(dialog).not.toBeVisible();
  }
  await action("Accept", "Confirm accept");
  await action("Dispatch", "Confirm dispatch", async () => {
    await dialog.getByLabel("Driver", { exact: true }).selectOption("d3");
    await dialog.getByLabel("Equipment", { exact: true }).selectOption("e3");
  });
  await action("Pickup", "Confirm pickup");
  await action("Location", "Confirm location", async () =>
    dialog.getByLabel("Reported location").fill("Flagstaff, AZ"),
  );
  await action("Deliver", "Confirm deliver", async () =>
    dialog.getByLabel("Proof of delivery reference").fill("SIGNED-POD-E2E"),
  );
  await open();
  await expect(
    dialog
      .getByLabel("Shipment documents")
      .getByText("SIGNED-POD-E2E", { exact: true }),
  ).toBeVisible();
  await dialog.getByRole("button", { name: "Close dialog" }).click();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Freight audit" })
    .click();
  await page.getByRole("button", { name: "Add invoice" }).click();
  const number = `INV-${Date.now()}`;
  await dialog.getByLabel("Carrier invoice number").fill(number);
  await dialog
    .getByLabel("Load", { exact: true })
    .selectOption({ label: `${reference} · agreed $2,000.01` });
  await dialog.getByLabel("Invoice total (shipment currency)").fill("2000.01");
  await dialog.getByRole("button", { name: "Save record" }).click();
  const invoice = page.getByRole("row").filter({ hasText: number });
  await expect(
    invoice.getByRole("button", { name: "Approve", exact: true }),
  ).toBeDisabled();
  await invoice
    .getByRole("button", { name: "Review delivery evidence" })
    .click();
  await dialog.getByRole("button", { name: "Review external POD" }).click();
  await dialog
    .getByLabel("Evidence review note")
    .fill(
      "Checked the signed delivery receipt against this shipment and receiver.",
    );
  await dialog.getByRole("button", { name: "Record review" }).click();
  await expect(
    dialog.getByText("Reviewed delivery evidence", { exact: true }),
  ).toBeVisible();
  await dialog.getByRole("button", { name: "Close dialog" }).click();
  await invoice.getByRole("button", { name: "Approve", exact: true }).click();
  await invoice.getByRole("button", { name: "Prepare settlement" }).click();
  await expect(invoice.getByText("Settlement prepared")).toBeVisible();
  await page.reload();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /Load board/ })
    .click();
  await page.getByLabel("Search loads").fill(reference);
  await expect(
    page.getByRole("button", { name: reference, exact: true }),
  ).toBeVisible();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Integrations" })
    .click();
  await expect(page.getByText("pending_handoff", { exact: false })).toHaveCount(
    0,
  );
  await expect(
    page
      .locator(".handoff")
      .filter({ hasText: reference })
      .getByText("Pending Handoff", { exact: true }),
  ).toBeVisible();
  expect(errors).toEqual([]);
  if (process.env.FREIGHT_E2E_SCREENSHOTS === "true") {
    await page
      .getByRole("navigation")
      .getByRole("button", { name: "Overview", exact: true })
      .click();
    await page.screenshot({
      path: "../docs/screenshots/desktop.png",
      fullPage: true,
    });
  }
});

test("stored POD keeps exact file bytes and requires an auditable finance review", async ({
  page,
}, testInfo) => {
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned synthetic workspace",
  );
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /^Load board/ })
    .click();
  await page.getByLabel("Search loads").fill("FR-1045");
  await page.getByRole("button", { name: "FR-1045", exact: true }).click();
  const dialog = page.getByRole("dialog"),
    filename = "POD réception.png";
  const bytes = Buffer.from(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/l3sAAAAASUVORK5CYII=",
    "base64",
  );
  await dialog
    .getByLabel("Shipment document file")
    .setInputFiles({ name: filename, mimeType: "image/png", buffer: bytes });
  await dialog
    .getByRole("button", { name: "Upload document", exact: true })
    .click();
  const card = dialog.getByRole("article").filter({
    has: page.getByRole("link", {
      name: `Download ${filename}`,
      exact: true,
    }),
  });
  await expect(card).toBeVisible();
  await expect(card.getByText("Pending Review", { exact: true })).toBeVisible();
  await expect(
    card.getByText(createHash("sha256").update(bytes).digest("hex"), {
      exact: true,
    }),
  ).toBeVisible();
  const downloadPromise = page.waitForEvent("download");
  await card
    .getByRole("link", { name: `Download ${filename}`, exact: true })
    .click();
  const download = await downloadPromise;
  expect(download.suggestedFilename()).toBe(filename);
  const downloaded = testInfo.outputPath("retained-pod.png");
  await download.saveAs(downloaded);
  expect(await readFile(downloaded)).toEqual(bytes);
  // Qualify the production gateway above Next's former 1 MiB JSON body limit.
  const large = Buffer.concat([
    Buffer.from("%PDF-1.4\n%"),
    Buffer.alloc(1100000, 0x20),
    Buffer.from("\n%%EOF"),
  ]);
  const largeUpload = await page.request.post(
    "/api/freight/loads/load_7/documents?kind=other&filename=large-transport-fixture.pdf",
    {
      headers: {
        Origin: new URL(page.url()).origin,
        "Content-Type": "application/pdf",
        "Idempotency-Key": randomUUID(),
      },
      data: large,
    },
  );
  expect(largeUpload.ok(), await largeUpload.text()).toBe(true);
  const largeDocument = (await largeUpload.json()).data;
  const largeDownload = await page.request.get(
    `/api/freight/documents/${largeDocument.id}/content`,
  );
  expect(largeDownload.ok()).toBe(true);
  expect(await largeDownload.body()).toEqual(large);
  await card.getByRole("button", { name: "Use as POD", exact: true }).click();
  await expect(
    card.getByRole("button", { name: "Attached as POD", exact: true }),
  ).toBeDisabled();
  await expect(
    dialog.getByText("Finance review required", { exact: true }),
  ).toBeVisible();
  await card
    .getByRole("button", { name: "Review document", exact: true })
    .click();
  await dialog.getByLabel("Evidence review decision").selectOption("rejected");
  await dialog
    .getByLabel("Evidence review note")
    .fill("Receiver confirmation needs an independent check before approval.");
  await dialog
    .getByRole("button", { name: "Record review", exact: true })
    .click();
  await expect(card.getByRole("status")).toHaveText("Rejected");
  await card
    .getByRole("button", { name: "Review document", exact: true })
    .click();
  await dialog
    .getByLabel("Evidence review note")
    .fill(
      "Receiver confirmation independently checked against the original shipment record.",
    );
  await dialog
    .getByRole("button", { name: "Record review", exact: true })
    .click();
  await expect(
    dialog.getByText("Reviewed delivery evidence", { exact: true }),
  ).toBeVisible();
  await card.getByText("Review history (2)", { exact: true }).click();
  await expect(
    card.getByText(/Receiver confirmation needs an independent check/),
  ).toBeVisible();
  await expect(
    card.getByText(/Receiver confirmation independently checked/),
  ).toBeVisible();
  if (process.env.FREIGHT_E2E_SCREENSHOTS === "true")
    await page.screenshot({
      path: "../docs/screenshots/documents.png",
      fullPage: true,
    });
  await page.reload();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /^Load board/ })
    .click();
  await page.getByLabel("Search loads").fill("FR-1045");
  await page.getByRole("button", { name: "FR-1045", exact: true }).click();
  await expect(
    page.getByRole("article").filter({
      has: page.getByRole("link", {
        name: `Download ${filename}`,
        exact: true,
      }),
    }),
  ).toBeVisible();
  await expect(
    page.getByText("Reviewed delivery evidence", { exact: true }),
  ).toBeVisible();
  await page.setViewportSize({ width: 390, height: 844 });
  const mobileReview = page
    .getByRole("article")
    .filter({
      has: page.getByRole("link", {
        name: `Download ${filename}`,
        exact: true,
      }),
    })
    .getByRole("button", { name: "Review document", exact: true });
  await mobileReview.scrollIntoViewIfNeeded();
  expect(
    await mobileReview.evaluate((element) =>
      parseFloat(getComputedStyle(element).fontSize),
    ),
  ).toBeGreaterThan(0);
  await mobileReview.click();
  await expect(page.getByLabel("Evidence review decision")).toBeFocused();
  await expect(page.getByLabel("Evidence review note")).toBeVisible();
  expect(
    await page
      .getByRole("dialog")
      .evaluate((element) => element.scrollWidth <= element.clientWidth),
  ).toBe(true);
  if (process.env.FREIGHT_E2E_SCREENSHOTS === "true")
    await page.screenshot({
      path: "../docs/screenshots/documents-mobile.png",
      fullPage: true,
    });
});

test("finance issues customer billing, reconciles bank receipts and carrier payouts, and preserves corrections", async ({
  page,
}, testInfo) => {
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires an owned synthetic finance workspace",
  );
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Freight audit", exact: true })
    .click();
  await page
    .getByRole("tab", { name: "Customer billing", exact: true })
    .click();
  await page
    .getByRole("button", { name: "Edit billing identity", exact: true })
    .click();
  await page
    .getByLabel("Issuing legal name")
    .fill("Synthetic Freight Billing Test LLC");
  await page
    .getByLabel("Issuing address")
    .fill("500 Synthetic Billing Road, Demo City, OR 97000, US");
  await page.getByLabel("Billing contact").fill("billing@example.test");
  await page
    .getByLabel("Payment instructions")
    .fill(
      "Synthetic test only. Use separately confirmed payment instructions.",
    );
  await page
    .getByRole("button", { name: "Save billing identity", exact: true })
    .click();
  await page
    .getByRole("button", { name: "Draft customer invoice", exact: true })
    .click();
  const dialog = page.getByRole("dialog"),
    number = `BILL-E2E-${Date.now()}`;
  await dialog.getByLabel("Delivered shipment").selectOption("load_6");
  await dialog.getByLabel("Customer invoice number").fill(number);
  await dialog
    .getByLabel("Bill-to address")
    .fill(
      "Northstar Retail Accounts Payable, 99 Synthetic Boulevard, Demo City, OR",
    );
  await dialog
    .getByRole("button", { name: "Create draft", exact: true })
    .click();
  const billRow = page.getByRole("row").filter({ hasText: number });
  await expect(billRow.getByText("Draft", { exact: true })).toBeVisible();
  await billRow
    .getByRole("button", { name: "Issue invoice", exact: true })
    .click();
  await dialog
    .getByRole("button", { name: "Confirm issue", exact: true })
    .click();
  await expect(billRow.getByText("Issued", { exact: true })).toBeVisible();
  await billRow
    .getByRole("button", { name: "Credit note", exact: true })
    .click();
  await dialog.getByLabel("Credit amount (USD)").fill("90.00");
  await dialog
    .getByLabel("Finance reason")
    .fill("Customer agreed adjustment for the shipment service.");
  await dialog
    .getByRole("button", { name: "Post credit note", exact: true })
    .click();
  const recordReceipt = async (amount, reference) => {
    await billRow
      .getByRole("button", { name: "Record receipt", exact: true })
      .click();
    await dialog.getByLabel("Bank amount (USD)").fill(amount);
    await dialog.getByLabel(`Allocation amount ${number}`).fill(amount);
    await dialog.getByLabel("Cleared bank reference").fill(reference);
    await dialog
      .getByLabel("Reconciliation note")
      .fill("Matched cleared customer deposit against external bank record.");
    await dialog
      .getByRole("button", { name: "Record reconciliation", exact: true })
      .click();
    await expect(dialog).not.toBeVisible();
  };
  const firstReference = `BANK-IN-1-${Date.now()}`,
    secondReference = `BANK-IN-2-${Date.now()}`;
  await recordReceipt("400.00", firstReference);
  await expect(
    billRow.getByText("Partially Paid", { exact: true }),
  ).toBeVisible();
  await recordReceipt("1000.00", secondReference);
  await expect(billRow.getByText("Paid", { exact: true })).toBeVisible();
  const downloadPromise = page.waitForEvent("download");
  await billRow
    .getByRole("link", { name: `Download invoice ${number}`, exact: true })
    .click();
  const download = await downloadPromise;
  const output = testInfo.outputPath("customer-invoice.html");
  await download.saveAs(output);
  const statement = await readFile(output, "utf8");
  expect(statement).toContain("USD 1490.00");
  expect(statement).toContain("Synthetic Freight Billing Test LLC");
  expect(statement).toContain(
    "Northstar Retail Accounts Payable, 99 Synthetic Boulevard",
  );
  expect(statement).toContain("USD 1400.00");
  expect(statement).toContain("USD 90.00");
  expect(statement).not.toContain("1180.00");
  if (process.env.FREIGHT_E2E_SCREENSHOTS === "true")
    await page.evaluate(() => window.scrollTo(0, 0));
  if (process.env.FREIGHT_E2E_SCREENSHOTS === "true")
    await page.screenshot({
      path: "../docs/screenshots/customer-billing.png",
      fullPage: true,
    });
  await page.getByRole("tab", { name: "Carrier audit", exact: true }).click();
  const carrierInvoice = page.getByRole("row").filter({ hasText: "INV-2084" });
  await carrierInvoice
    .getByRole("button", { name: "Approve", exact: true })
    .click();
  await carrierInvoice
    .getByRole("button", { name: "Prepare settlement", exact: true })
    .click();
  await page
    .getByRole("tab", { name: "Bank reconciliation", exact: true })
    .click();
  await page
    .getByRole("button", { name: "Record bank transaction", exact: true })
    .click();
  await dialog.getByLabel("Transaction direction").selectOption("payout");
  await dialog.getByLabel("Counterparty").selectOption("ca1");
  await dialog.getByLabel("Allocate FR-1046", { exact: true }).check();
  await dialog.getByLabel("Bank amount (USD)").fill("1180.00");
  await dialog
    .getByLabel("Cleared bank reference")
    .fill(`BANK-OUT-${Date.now()}`);
  await dialog
    .getByLabel("Reconciliation note")
    .fill("Confirmed carrier wire cleared against bank statement.");
  await dialog
    .getByRole("button", { name: "Record reconciliation", exact: true })
    .click();
  await expect(dialog).not.toBeVisible();
  const bankRow = page.getByRole("row").filter({ hasText: secondReference });
  await bankRow
    .getByRole("button", { name: "Reverse reconciliation", exact: true })
    .click();
  await dialog
    .getByLabel("Finance reason")
    .fill(
      "Correct an incorrect bank allocation entry; original deposit was independently confirmed.",
    );
  await dialog
    .getByRole("button", { name: "Confirm reversal", exact: true })
    .click();
  await expect(bankRow.getByText("Reversed", { exact: true })).toBeVisible();
  await bankRow
    .getByRole("button", { name: "Correct reversed entry", exact: true })
    .click();
  await dialog
    .getByLabel("Reconciliation note")
    .fill(
      "Re-recorded the confirmed cleared deposit with explicit correction lineage.",
    );
  await dialog
    .getByRole("button", { name: "Record reconciliation", exact: true })
    .click();
  await expect(dialog).not.toBeVisible();
  const workspace = await (
    await page.request.get("/api/freight/workspace")
  ).json();
  const bill = workspace.bills.find((value) => value.number === number);
  expect(bill.status).toBe("paid");
  expect(bill.totals.balance_cents).toBe(0);
  const corrections = workspace.cash_entries.filter(
    (value) => value.external_reference === secondReference,
  );
  expect(corrections).toHaveLength(2);
  expect(corrections[0].corrected_by_id).toBe(corrections[1].id);
  expect(corrections[1].corrects_id).toBe(corrections[0].id);
  const lines = workspace.journal
    .flatMap((entry) => entry.lines)
    .filter((line) => line.load_id === "load_6");
  const net = (account) =>
    lines
      .filter((line) => line.account === account)
      .reduce((sum, line) => sum + line.debit_cents - line.credit_cents, 0);
  expect(net("cash")).toBe(22000);
  expect(net("accounts_receivable")).toBe(0);
  expect(net("carrier_payable")).toBe(0);
  for (const entry of workspace.journal)
    expect(
      entry.lines.reduce(
        (sum, line) => sum + line.debit_cents - line.credit_cents,
        0,
      ),
    ).toBe(0);
  await page.getByRole("tab", { name: "Journal", exact: true }).click();
  const exportDownload = page.waitForEvent("download");
  await page
    .getByRole("link", { name: "Export journal CSV", exact: true })
    .click();
  expect((await exportDownload).suggestedFilename()).toBe(
    "freight-journal.csv",
  );
  if (process.env.FREIGHT_E2E_SCREENSHOTS === "true")
    await page.evaluate(() => window.scrollTo(0, 0));
  if (process.env.FREIGHT_E2E_SCREENSHOTS === "true")
    await page.screenshot({
      path: "../docs/screenshots/financial-ledger.png",
      fullPage: true,
    });
  await page.setViewportSize({ width: 390, height: 844 });
  await page
    .getByRole("tab", { name: "Customer billing", exact: true })
    .click();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBe(true);
});

test("operator books facility windows, records gate evidence, and corrects times without inventing shipment progress", async ({
  page,
}) => {
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned synthetic stop workspace",
  );
  const origin = new URL(page.url()).origin;
  const post = async (path, body) => {
    const response = await page.request.post(`/api/freight/${path}`, {
      headers: { Origin: origin, "Idempotency-Key": randomUUID() },
      data: body,
    });
    expect(response.ok(), await response.text()).toBe(true);
    return (await response.json()).data;
  };
  const pickup = new Date(Date.now() - 2 * 86400000).toISOString().slice(0, 10),
    delivery = new Date(Date.now() - 86400000).toISOString().slice(0, 10),
    reference = `E2E-APPT-${Date.now()}`;
  const created = await post("loads", {
    reference,
    customer_id: "c1",
    mode: "FTL",
    equipment: "Dry van",
    commodity: "Appointment fixtures",
    weight_lbs: 1000,
    miles: 173,
    buy_cents: 120000,
    sell_cents: 150000,
    origin: {
      facility: "Portland dock",
      city: "Portland",
      state: "OR",
      date: pickup,
      address_lines: ["15 Dock Street", "Gate 2"],
      postal_code: "97201",
      country: "US",
      contact: "Shipping desk",
      phone: "+1 503 555 0100",
      instructions: "Driver checks in at gate 2",
    },
    destination: {
      facility: "Seattle receiving",
      city: "Seattle",
      state: "WA",
      date: delivery,
      address_lines: ["25 Receiving Ave"],
      postal_code: "98101",
      country: "US",
    },
  });
  const snapshot = async () => {
    const response = await page.request.get("/api/freight/workspace");
    expect(response.ok()).toBe(true);
    return response.json();
  };
  const open = async () => {
    await page.reload();
    if (
      await page.getByRole("button", { name: "Toggle navigation" }).isVisible()
    )
      await page.getByRole("button", { name: "Toggle navigation" }).click();
    await page
      .getByRole("navigation")
      .getByRole("button", { name: /Load board/ })
      .click();
    await page.getByRole("button", { name: reference, exact: true }).click();
  };
  await open();
  const dialog = page.getByRole("dialog"),
    panel = dialog.getByLabel("Stop appointments and gate times"),
    form = panel.getByRole("form", { name: "Stop evidence form" });
  await expect(
    panel.getByText("15 Dock Street", { exact: true }),
  ).toBeVisible();
  await expect(
    panel.getByText("Driver checks in at gate 2", { exact: true }),
  ).toBeVisible();
  const fillAppointment = async (date, start, end, ref) => {
    await form.getByLabel("Appointment window start").fill(`${date}T${start}`);
    await form.getByLabel("Appointment window end").fill(`${date}T${end}`);
    await form.getByLabel("Facility timezone").fill("UTC");
    await form
      .getByLabel("Appointment facility contact")
      .fill("Facility scheduler");
    await form.getByLabel("Stop confirmation reference").fill(ref);
    await form
      .getByLabel("Stop evidence note")
      .fill("Facility and carrier confirmed this window directly.");
  };
  await panel
    .getByRole("button", { name: "Book pickup appointment", exact: true })
    .click();
  await expect(form.getByLabel("Appointment window start")).toBeFocused();
  await fillAppointment(pickup, "08:00", "09:00", "PICKUP-1");
  await form.getByLabel("Facility timezone").fill("America/Invalid_Zone");
  await form
    .getByRole("button", { name: "Save stop evidence", exact: true })
    .click();
  await expect(panel.getByRole("alert")).toContainText("valid IANA timezone");
  await form.getByLabel("Facility timezone").fill("UTC");
  await form
    .getByRole("button", { name: "Save stop evidence", exact: true })
    .click();
  await expect(form).not.toBeVisible();
  await panel
    .getByRole("button", { name: "Reschedule pickup", exact: true })
    .click();
  await fillAppointment(pickup, "09:00", "10:00", "PICKUP-2");
  await form
    .getByRole("button", { name: "Save stop evidence", exact: true })
    .click();
  await expect(form).not.toBeVisible();
  await panel
    .getByRole("button", { name: "Book delivery appointment", exact: true })
    .click();
  await fillAppointment(delivery, "11:00", "12:00", "DELIVERY-1");
  await form
    .getByRole("button", { name: "Save stop evidence", exact: true })
    .click();
  await expect(form).not.toBeVisible();
  await panel
    .getByRole("button", { name: "Cancel delivery appointment", exact: true })
    .click();
  await form
    .getByLabel("Appointment cancellation reason")
    .fill("Receiver withdrew the initial booking; carrier informed.");
  await form
    .getByRole("button", {
      name: "Record appointment cancellation",
      exact: true,
    })
    .click();
  await expect(form).not.toBeVisible();
  await panel
    .getByRole("button", { name: "Book delivery appointment", exact: true })
    .click();
  await fillAppointment(delivery, "11:00", "12:00", "DELIVERY-2");
  await form
    .getByRole("button", { name: "Save stop evidence", exact: true })
    .click();
  await expect(form).not.toBeVisible();
  let w = await snapshot(),
    current = w.loads.find((v) => v.id === created.id);
  await post(`loads/${created.id}/commands`, {
    action: "tender",
    expected_version: current.version,
    carrier_id: "ca4",
  });
  w = await snapshot();
  current = w.loads.find((v) => v.id === created.id);
  await post(`loads/${created.id}/commands`, {
    action: "accept",
    expected_version: current.version,
  });
  await open();
  const fillGate = async (date, time, ref, note) => {
    await form.getByLabel("Actual gate local time").fill(`${date}T${time}`);
    await form.getByLabel("Facility timezone").fill("UTC");
    await form.getByLabel("Stop confirmation reference").fill(ref);
    await form.getByLabel("Stop evidence note").fill(note);
    await form
      .getByRole("button", { name: "Save stop evidence", exact: true })
      .click();
    await expect(form).not.toBeVisible();
  };
  await panel
    .getByRole("button", { name: "Record pickup arrival", exact: true })
    .click();
  await fillGate(
    pickup,
    "10:10",
    "GATE-IN-1",
    "Gate clerk confirmed actual arrival.",
  );
  await expect(
    panel.getByText("Late · 10 min after window", { exact: true }),
  ).toBeVisible();
  await expect(
    panel.getByRole("button", { name: "Reschedule pickup", exact: true }),
  ).toHaveCount(0);
  w = await snapshot();
  expect(w.loads.find((v) => v.id === created.id).status).toBe("accepted");
  await panel
    .getByRole("button", { name: "Correct pickup arrival", exact: true })
    .click();
  await fillGate(
    pickup,
    "10:05",
    "GATE-IN-CORRECTED",
    "Gate clerk corrected the ledger timestamp by five minutes.",
  );
  await expect(
    panel.getByText("Late · 5 min after window", { exact: true }),
  ).toBeVisible();
  w = await snapshot();
  const tender = w.tenders.find(
    (v) => v.id === w.loads.find((v) => v.id === created.id).tender_id,
  );
  await post("events", {
    event_id: randomUUID(),
    event_type: "edi.shipment_status.received",
    occurred_at: new Date().toISOString(),
    payload: {
      shipment_id: tender.shipment_reference,
      partner_id: "brex",
      carrier_scac: "BREX",
      latest_status: "AF",
      events: [{ status_code: "AF" }],
    },
  });
  await open();
  await panel
    .getByRole("button", { name: "Record pickup departure", exact: true })
    .click();
  await fillGate(
    pickup,
    "10:45",
    "GATE-OUT-1",
    "Gate ledger confirms the loaded departure time.",
  );
  await expect(
    panel.getByText("40 min recorded dwell", { exact: true }),
  ).toBeVisible();
  await page.setViewportSize({ width: 390, height: 844 });
  await panel
    .getByRole("button", { name: "Record delivery arrival", exact: true })
    .click();
  expect(
    await dialog.evaluate(
      (element) => element.scrollWidth <= element.clientWidth,
    ),
  ).toBe(true);
  await fillGate(
    delivery,
    "12:15",
    "DELIVERY-IN-1",
    "Receiving desk confirmed arrival while unloading is pending.",
  );
  await expect(
    panel.getByText("Late · 15 min after window", { exact: true }),
  ).toBeVisible();
  await expect(
    panel.getByRole("button", {
      name: "Record delivery departure",
      exact: true,
    }),
  ).toHaveCount(0);
  w = await snapshot();
  current = w.loads.find((v) => v.id === created.id);
  expect(current.status).toBe("in_transit");
  expect(current.pod_reference).toBeNull();
  expect(w.invoices.filter((v) => v.load_id === created.id)).toHaveLength(0);
  expect(w.appointments.filter((v) => v.load_id === created.id)).toHaveLength(
    4,
  );
  const gates = w.stop_events.filter((v) => v.load_id === created.id);
  expect(gates).toHaveLength(4);
  expect(gates.filter((v) => v.supersedes_id)).toHaveLength(1);
  await panel.getByText(/Appointment and gate evidence history/).click();
  await expect(
    panel
      .getByText("Facility contact: Facility scheduler", { exact: true })
      .first(),
  ).toBeVisible();
  if (process.env.FREIGHT_E2E_SCREENSHOTS === "true") {
    await page.setViewportSize({ width: 1280, height: 1100 });
    await dialog.evaluate((element) => {
      const section = element.querySelector(".stop-execution");
      element.scrollTop +=
        section.getBoundingClientRect().top -
        element.getBoundingClientRect().top -
        24;
    });
    await page.screenshot({
      path: "../docs/screenshots/stop-execution.png",
      fullPage: false,
    });
  }
});

test("operations proposes documented charges and finance carries approved amounts into immutable billing and settlement", async ({
  page,
}) => {
  test.setTimeout(90000);
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned synthetic charge workspace",
  );
  const origin = new URL(page.url()).origin;
  const post = async (path, data) => {
    const response = await page.request.post(`/api/freight/${path}`, {
      headers: { Origin: origin, "Idempotency-Key": randomUUID() },
      data,
    });
    expect(response.ok(), await response.text()).toBe(true);
    return (await response.json()).data;
  };
  const snapshot = async () => {
    const response = await page.request.get("/api/freight/workspace");
    expect(response.ok()).toBe(true);
    return response.json();
  };
  const login = async (role) => {
    const response = await page.request.post("/api/session", {
      headers: { Origin: origin },
      data: { token: `freight-test-${role}-credential-at-least-32-chars` },
    });
    expect(response.ok()).toBe(true);
    await page.goto("/");
    await expect(page.getByLabel("Workspace access role")).toHaveText(
      `${role[0].toUpperCase()}${role.slice(1)} access`,
    );
  };
  const reference = `E2E-CHARGES-${Date.now()}`,
    invoiceNumber = `CARRIER-${Date.now()}`,
    billNumber = `CUSTOMER-${Date.now()}`,
    today = new Date().toISOString().slice(0, 10);
  const customer = await post("customers", {
    name: "Charge workflow customer",
    contact: "billing@example.test",
    billing_address: "25 Receiver Way, Seattle WA 98101",
  });
  let created = await post("loads", {
    reference,
    customer_id: customer.id,
    mode: "FTL",
    equipment: "Dry van",
    commodity: "Charge fixtures",
    weight_lbs: 1000,
    miles: 173,
    buy_cents: 120001,
    sell_cents: 150025,
    origin: {
      facility: "Origin dock",
      city: "Portland",
      state: "OR",
      date: today,
      address_lines: ["15 Dock Street"],
      postal_code: "97201",
      country: "US",
    },
    destination: {
      facility: "Receiver",
      city: "Seattle",
      state: "WA",
      date: today,
      address_lines: ["25 Receiver Way"],
      postal_code: "98101",
      country: "US",
    },
  });
  created = await post(`loads/${created.id}/commands`, {
    action: "tender",
    expected_version: created.version,
    carrier_id: "ca4",
  });
  created = await post(`loads/${created.id}/commands`, {
    action: "accept",
    expected_version: created.version,
  });
  let w = await snapshot();
  const attempt = w.tenders.find((v) => v.id === created.tender_id);
  await post("events", {
    event_id: randomUUID(),
    event_type: "edi.shipment_status.received",
    occurred_at: new Date().toISOString(),
    payload: {
      shipment_id: attempt.shipment_reference,
      partner_id: "brex",
      carrier_scac: "BREX",
      latest_status: "D1",
      events: [{ status_code: "AF" }, { status_code: "D1" }],
    },
  });
  await login("operations");
  w = await snapshot();
  created = w.loads.find((v) => v.id === created.id);
  await post(`loads/${created.id}/commands`, {
    action: "attach_pod",
    expected_version: created.version,
    pod_reference: "External signed receiver evidence CHARGES",
  });
  const proof = Buffer.from(
    "%PDF-1.4\n1 0 obj << /Type /Catalog >> endobj\n%%EOF\n",
  );
  const uploaded = await page.request.post(
    `/api/freight/loads/${created.id}/documents?kind=rate_confirmation&filename=additional-service.pdf`,
    {
      headers: {
        Origin: origin,
        "Idempotency-Key": randomUUID(),
        "Content-Type": "application/pdf",
      },
      data: proof,
    },
  );
  expect(uploaded.ok(), await uploaded.text()).toBe(true);
  const document = (await uploaded.json()).data;
  const openLoad = async () => {
    await page
      .getByRole("navigation")
      .getByRole("button", { name: /Load board/ })
      .click();
    await page.getByRole("button", { name: reference, exact: true }).click();
  };
  await page.reload();
  await openLoad();
  const dialog = page.getByRole("dialog"),
    panel = dialog.getByLabel("Additional freight charges"),
    form = panel.getByRole("form", { name: "Freight charge form" });
  const propose = async (
    side,
    amount,
    ref,
    description,
    kind = "detention",
  ) => {
    await panel
      .getByRole("button", { name: "Propose charge", exact: true })
      .click();
    await form.getByLabel("Charge agreement side").selectOption(side);
    await form.getByLabel("Charge kind", { exact: true }).selectOption(kind);
    await form.getByLabel("Charge amount (USD)", { exact: true }).fill(amount);
    await form.getByLabel("Charge evidence reference").fill(ref);
    await form.getByLabel("Charge service description").fill(description);
    await form
      .getByLabel("Charge supporting document")
      .selectOption(document.id);
    await form
      .getByRole("button", { name: "Save charge decision", exact: true })
      .click();
    await expect(form).not.toBeVisible();
  };
  await propose(
    "buy",
    "100.01",
    "CARRIER-AGREEMENT-1",
    "Carrier additional wait cost internal",
  );
  await propose(
    "sell",
    "150.01",
    "CUSTOMER-AGREEMENT-1",
    "Additional loading service",
  );
  await expect(
    panel.getByRole("button", { name: "Approve carrier charge", exact: true }),
  ).toHaveCount(0);
  w = await snapshot();
  const buy = w.charges.find(
    (v) => v.load_id === created.id && v.side === "buy",
  );
  expect(buy.document_sha256).toBe(document.sha256);
  const denied = await page.request.post(
    `/api/freight/charges/${buy.id}/review`,
    {
      headers: { Origin: origin, "Idempotency-Key": randomUUID() },
      data: {
        expected_load_version: w.loads.find((v) => v.id === created.id).version,
        expected_version: buy.version,
        decision: "approved",
        counterparty_contact: "Spoofed",
        confirmation_reference: "No permission",
        note: "Operations cannot approve",
      },
    },
  );
  expect(denied.status()).toBe(403);
  await login("finance");
  w = await snapshot();
  created = w.loads.find((v) => v.id === created.id);
  await post(`loads/${created.id}/pod/review`, {
    expected_version: created.version,
    decision: "accepted",
    note: "Inspected actual external receiver evidence for this shipment",
  });
  await post("finance/profile", {
    expected_version: w.billing_profile?.version || 0,
    legal_name: "Charge qualification issuer",
    address: "100 Billing Way, Portland OR 97201",
    contact: "finance@example.test",
    payment_instructions: "Use the externally verified remittance channel",
  });
  const invoice = await post("invoices", {
    number: invoiceNumber,
    load_id: created.id,
    amount_cents: 130002,
    currency: "USD",
  });
  expect(invoice.status).toBe("exception");
  expect(invoice.variance_cents).toBe(10001);
  const bill = await post("bills", {
    number: billNumber,
    load_id: created.id,
    due_date: today,
  });
  expect(bill.snapshot.amount_cents).toBe(150025);
  await page.reload();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Freight audit", exact: true })
    .click();
  const queue = page.getByLabel("Charges awaiting finance review");
  await expect(queue).toContainText(reference);
  await queue
    .getByRole("button")
    .filter({ hasText: "Customer charge" })
    .filter({ hasText: reference })
    .click();
  const agree = async (button, contact, ref, note) => {
    await button.click();
    await form.getByLabel("Charge counterparty contact").fill(contact);
    await form.getByLabel("Charge agreement confirmation").fill(ref);
    await form.getByLabel("Charge decision note").fill(note);
    await form
      .getByRole("button", { name: "Save charge decision", exact: true })
      .click();
    await expect(form).not.toBeVisible();
  };
  await agree(
    panel.getByRole("button", { name: "Approve carrier charge", exact: true }),
    "Carrier dispatcher",
    "CARRIER-CONF-1",
    "Reviewed the original service agreement and verified the exact carrier cost.",
  );
  await agree(
    panel.getByRole("button", { name: "Approve customer charge", exact: true }),
    "Customer transport manager",
    "CUSTOMER-CONF-1",
    "Customer explicitly agreed the additional loading service amount.",
  );
  w = await snapshot();
  let currentBill = w.bills.find((v) => v.id === bill.id);
  expect(currentBill.version).toBe(2);
  expect(currentBill.snapshot.amount_cents).toBe(165026);
  expect(w.invoices.find((v) => v.id === invoice.id).amount_cents).toBe(130002);
  expect(w.invoices.find((v) => v.id === invoice.id).status).toBe("matched");
  await expect(panel.getByText("$350.24", { exact: true })).toBeVisible();
  const stale = await page.request.post(`/api/freight/bills/${bill.id}/issue`, {
    headers: { Origin: origin, "Idempotency-Key": randomUUID() },
    data: { expected_version: 1 },
  });
  expect(stale.status()).toBe(409);
  if (process.env.FREIGHT_E2E_SCREENSHOTS === "true") {
    await page.setViewportSize({ width: 1280, height: 1000 });
    await dialog.evaluate((element) => {
      const section = element.querySelector(".shipment-charges");
      element.scrollTop +=
        section.getBoundingClientRect().top -
        element.getBoundingClientRect().top -
        24;
    });
    await page.screenshot({
      path: "../docs/screenshots/freight-charges.png",
      fullPage: false,
    });
  }
  await page.setViewportSize({ width: 390, height: 844 });
  await propose(
    "buy",
    "5.01",
    "FUEL-CORRECTION",
    "Carrier fuel proposal entered in error",
    "fuel",
  );
  expect(
    await dialog.evaluate(
      (element) => element.scrollWidth <= element.clientWidth,
    ),
  ).toBe(true);
  const fuel = panel.getByLabel("Charge FUEL-CORRECTION buy");
  await fuel
    .getByRole("button", { name: "Reject charge", exact: true })
    .click();
  await form
    .getByLabel("Charge decision note")
    .fill(
      "Original amount was incorrect; retain it and request a corrected proposal.",
    );
  await form
    .getByRole("button", { name: "Save charge decision", exact: true })
    .click();
  await expect(form).not.toBeVisible();
  await fuel
    .getByRole("button", { name: "Correct charge proposal", exact: true })
    .click();
  await expect(form.getByLabel("Charge evidence reference")).toHaveValue(
    "FUEL-CORRECTION",
  );
  await expect(form.getByLabel("Charge evidence reference")).toHaveAttribute(
    "readonly",
    "",
  );
  await form.getByLabel("Charge amount (USD)", { exact: true }).fill("5.02");
  await form
    .getByRole("button", { name: "Save charge decision", exact: true })
    .click();
  await expect(form).not.toBeVisible();
  const currentFuel = fuel.filter({
    has: page.getByRole("button", {
      name: "Approve carrier charge",
      exact: true,
    }),
  });
  await agree(
    currentFuel.getByRole("button", {
      name: "Approve carrier charge",
      exact: true,
    }),
    "Carrier accounting",
    "FUEL-CONF-2",
    "Carrier verified the corrected amount.",
  );
  const approvedFuel = fuel.filter({
    has: page.getByRole("button", {
      name: "Void charge approval",
      exact: true,
    }),
  });
  await agree(
    approvedFuel.getByRole("button", {
      name: "Void charge approval",
      exact: true,
    }),
    "Carrier accounting",
    "FUEL-RETRACTED-3",
    "Carrier confirmed this fuel charge should not apply; retain the prior approval.",
  );
  await expect(panel.getByText("$350.24", { exact: true })).toBeVisible();
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.getByLabel("Close dialog").click();
  const carrierRow = page.getByRole("row").filter({ hasText: invoiceNumber });
  await carrierRow
    .getByRole("button", { name: "Approve", exact: true })
    .click();
  await carrierRow
    .getByRole("button", { name: "Prepare settlement", exact: true })
    .click();
  await page
    .getByRole("tab", { name: "Customer billing", exact: true })
    .click();
  const billRow = page.getByRole("row").filter({ hasText: billNumber });
  await billRow
    .getByRole("button", { name: "Issue invoice", exact: true })
    .click();
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "Confirm issue", exact: true })
    .click();
  await expect(page.getByRole("dialog")).not.toBeVisible();
  const statement = await page.request.get(
    `/api/freight/bills/${bill.id}/statement`,
  );
  expect(statement.ok()).toBe(true);
  const html = await statement.text();
  expect(html).toContain("Additional loading service");
  expect(html).toContain("USD 1500.25");
  expect(html).toContain("USD 150.01");
  expect(html).toContain("USD 1650.26");
  expect(html).not.toContain("Carrier additional wait cost internal");
  expect(html).not.toContain("CARRIER-CONF-1");
  w = await snapshot();
  expect(w.settlements.find((v) => v.load_id === created.id).amount_cents).toBe(
    130002,
  );
  expect(w.charges.filter((v) => v.load_id === created.id)).toHaveLength(4);
  expect(w.loads.find((v) => v.id === created.id).buy_cents).toBe(120001);
  expect(w.loads.find((v) => v.id === created.id).sell_cents).toBe(150025);
});

test("finance posts supplemental carrier invoices and credits, reconciles an actual refund, and reverses without rewriting paid history", async ({
  page,
}) => {
  const origin = new URL(page.url()).origin,
    stamp = Date.now(),
    reference = `E2E-PAYABLE-${stamp}`,
    number = `AP-${stamp}`,
    supplement = `SUP-${stamp}`,
    credit = `CM-${stamp}`,
    today = new Date().toISOString().slice(0, 10);
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned synthetic payable workspace",
  );
  const post = async (path, data) => {
    const response = await page.request.post(`/api/freight/${path}`, {
      headers: { Origin: origin, "Idempotency-Key": randomUUID() },
      data,
    });
    expect(response.ok(), await response.text()).toBe(true);
    return (await response.json()).data;
  };
  const snapshot = async () => {
    const r = await page.request.get("/api/freight/workspace");
    expect(r.ok()).toBe(true);
    return r.json();
  };
  let created = await post("loads", {
    reference,
    customer_id: "c1",
    mode: "FTL",
    equipment: "Dry van",
    commodity: "Payable fixtures",
    weight_lbs: 1000,
    miles: 173,
    buy_cents: 100001,
    sell_cents: 150001,
    origin: {
      facility: "Origin dock",
      city: "Portland",
      state: "OR",
      date: today,
      address_lines: ["15 Dock Street"],
      postal_code: "97201",
      country: "US",
    },
    destination: {
      facility: "Receiver",
      city: "Seattle",
      state: "WA",
      date: today,
      address_lines: ["25 Receiver Way"],
      postal_code: "98101",
      country: "US",
    },
  });
  created = await post(`loads/${created.id}/commands`, {
    action: "tender",
    expected_version: created.version,
    carrier_id: "ca4",
  });
  created = await post(`loads/${created.id}/commands`, {
    action: "accept",
    expected_version: created.version,
  });
  let w = await snapshot();
  const tender = w.tenders.find((v) => v.id === created.tender_id);
  await post("events", {
    event_id: randomUUID(),
    event_type: "edi.shipment_status.received",
    occurred_at: new Date().toISOString(),
    payload: {
      shipment_id: tender.shipment_reference,
      partner_id: "brex",
      carrier_scac: "BREX",
      latest_status: "D1",
      events: [{ status_code: "AF" }, { status_code: "D1" }],
    },
  });
  w = await snapshot();
  created = await post(`loads/${created.id}/commands`, {
    action: "attach_pod",
    expected_version: w.loads.find((v) => v.id === created.id).version,
    pod_reference: "External signed POD payable qualification",
  });
  const login = await page.request.post("/api/session", {
    headers: { Origin: origin },
    data: { token: "freight-test-finance-credential-at-least-32-chars" },
  });
  expect(login.ok()).toBe(true);
  await post(`loads/${created.id}/pod/review`, {
    expected_version: created.version,
    decision: "accepted",
    note: "Inspected original external delivery evidence",
  });
  let invoice = await post("invoices", {
    load_id: created.id,
    number,
    amount_cents: 100001,
    currency: "USD",
  });
  invoice = await post(`invoices/${invoice.id}/approve`, {});
  const prepared = await post(`loads/${created.id}/settlement`, {});
  const cashInput = (target, amount, ref) => ({
    direction: "payout",
    currency: "USD",
    account: "operating-usd",
    external_reference: ref,
    amount_cents: amount,
    cleared_on: today,
    note: "Verified actual cleared payout",
    allocations: [
      {
        target_id: target.id,
        expected_version: target.version,
        amount_cents: amount,
      },
    ],
  });
  const originalPayment = await post(
    "cash",
    cashInput(prepared, 100001, `BASE-PAY-${stamp}`),
  );
  const integrationLogin = await page.request.post("/api/session", {
    headers: { Origin: origin },
    data: { token: "freight-test-integration-credential-at-least-32-chars" },
  });
  expect(integrationLogin.ok()).toBe(true);
  const additional = await post("events", {
    event_id: `extra-${stamp}`,
    event_type: "edi.freight_invoice.received",
    occurred_at: new Date().toISOString(),
    payload: {
      shipment_id: tender.shipment_reference,
      partner_id: "brex",
      carrier_scac: "BREX",
      invoice_number: supplement,
      net_amount: "125.01",
      currency: "USD",
      invoice_date: today,
    },
  });
  expect(additional.finance_review).toBe(true);
  const financeLogin = await page.request.post("/api/session", {
    headers: { Origin: origin },
    data: { token: "freight-test-finance-credential-at-least-32-chars" },
  });
  expect(financeLogin.ok()).toBe(true);
  await page.reload();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Freight audit", exact: true })
    .click();
  await page
    .getByRole("row")
    .filter({ hasText: number })
    .getByRole("button", { name: "Review shipment charges", exact: true })
    .click();
  const dialog = page.getByRole("dialog"),
    panel = dialog.getByLabel("Carrier payable adjustments"),
    form = panel.getByRole("form", { name: "Carrier payable form" });
  const draft = async (kind, num, amount) => {
    await panel
      .getByRole("button", { name: "Record carrier adjustment", exact: true })
      .click();
    await form.getByLabel("Carrier document treatment").selectOption(kind);
    await form.getByLabel("Carrier adjustment document number").fill(num);
    await form.getByLabel("Carrier document amount (USD)").fill(amount);
    await form
      .getByLabel("Carrier adjustment evidence reference")
      .fill(`Actual carrier document ${num}`);
    await form
      .getByLabel("Carrier adjustment description")
      .fill(
        kind === "credit_memo"
          ? "Carrier-issued credit for corrected unloading charge"
          : "Agreed additional unloading invoice",
      );
    await form
      .getByRole("button", { name: "Save adjustment draft", exact: true })
      .click();
    await expect(form).not.toBeVisible();
  };
  const confirm = async (button, note) => {
    await button.click();
    await form
      .getByLabel("Payable counterparty contact")
      .fill("Carrier accounting representative");
    await form
      .getByLabel("Payable confirmation reference")
      .fill(`External carrier case ${stamp}`);
    await form.getByLabel("Carrier payable decision note").fill(note);
    await form
      .getByRole("button", { name: "Record payable decision", exact: true })
      .click();
    await expect(form).not.toBeVisible();
  };
  await panel
    .getByRole("button", { name: "Classify carrier invoice", exact: true })
    .click();
  await expect(
    form.getByLabel("Carrier adjustment document number"),
  ).toHaveValue(supplement);
  await expect(
    form.getByLabel("Carrier document amount (USD)"),
  ).toHaveAttribute("readonly", "");
  await form
    .getByLabel("Carrier document treatment")
    .selectOption("supplemental_invoice");
  await form
    .getByLabel("Carrier adjustment description")
    .fill("Agreed additional unloading invoice from authenticated 210");
  await form
    .getByRole("button", { name: "Save adjustment draft", exact: true })
    .click();
  await expect(form).not.toBeVisible();
  await confirm(
    panel.getByRole("button", {
      name: "Approve and post adjustment",
      exact: true,
    }),
    "Verified this document is an agreed additional invoice rather than a replacement total.",
  );
  w = await snapshot();
  let target = w.settlements.find((v) => v.id === prepared.id);
  expect(target.amount_cents).toBe(100001);
  expect(target.total_amount_cents).toBe(112502);
  await post("cash", cashInput(target, 12501, `ADDITIONAL-PAY-${stamp}`));
  await page.setViewportSize({ width: 390, height: 844 });
  await draft("credit_memo", credit, "25.01");
  await confirm(
    panel.getByRole("button", {
      name: "Approve and post adjustment",
      exact: true,
    }),
    "Inspected actual carrier credit memo and confirmed the reduction after payment.",
  );
  await expect(
    panel.getByText("Carrier credit balance", { exact: true }),
  ).toBeVisible();
  await expect(panel.getByText("$25.01", { exact: true })).toBeVisible();
  expect(await dialog.evaluate((e) => e.scrollWidth <= e.clientWidth)).toBe(
    true,
  );
  if (process.env.FREIGHT_E2E_SCREENSHOTS === "true") {
    await page.setViewportSize({ width: 1280, height: 1000 });
    await dialog.evaluate((e) => {
      const section = e.querySelector(".carrier-payables");
      e.scrollTop +=
        section.getBoundingClientRect().top -
        e.getBoundingClientRect().top -
        24;
    });
    await page.screenshot({
      path: "../docs/screenshots/carrier-payables.png",
      fullPage: false,
    });
  }
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.getByLabel("Close dialog").click();
  await page
    .getByRole("tab", { name: "Bank reconciliation", exact: true })
    .click();
  await page
    .getByRole("button", { name: "Record bank transaction", exact: true })
    .click();
  await dialog
    .getByLabel("Transaction direction")
    .selectOption("carrier_refund");
  await dialog.getByLabel("Counterparty").selectOption("ca4");
  await dialog.getByLabel(`Allocate ${reference}`, { exact: true }).check();
  await dialog.getByLabel("Bank amount (USD)").fill("25.01");
  await dialog
    .getByLabel("Cleared bank reference")
    .fill(`ACTUAL-REFUND-${stamp}`);
  await dialog
    .getByLabel("Reconciliation note")
    .fill(
      "Matched an actual cleared carrier refund to this carrier credit balance.",
    );
  await dialog
    .getByRole("button", { name: "Record reconciliation", exact: true })
    .click();
  await expect(dialog).not.toBeVisible();
  w = await snapshot();
  target = w.settlements.find((v) => v.id === prepared.id);
  expect(target.balance_cents).toBe(0);
  expect(w.cash_entries.find((v) => v.id === originalPayment.id)).toEqual(
    originalPayment,
  );
  const { approval_control: invoiceControl, ...retainedInvoice } =
    w.invoices.find((v) => v.id === invoice.id);
  expect(invoiceControl.state).toBe("approved");
  expect(retainedInvoice).toEqual(invoice);
  expect(
    w.loads.find((v) => v.id === created.id).commercial.total_buy_cents,
  ).toBe(110001);
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /Load board/ })
    .click();
  await page.getByRole("button", { name: reference, exact: true }).click();
  await confirm(
    panel
      .locator(".payable-adjustment")
      .filter({ hasText: credit })
      .getByRole("button", { name: "Reverse adjustment posting", exact: true }),
    "Carrier formally withdrew its credit memo; retain the refund and reopen the resulting payable.",
  );
  w = await snapshot();
  expect(w.settlements.find((v) => v.id === prepared.id).balance_cents).toBe(
    2501,
  );
  expect(w.invoices.find((v) => v.id === invoice.id).amount_cents).toBe(100001);
  const record = w.carrier_adjustments.find((v) => v.number === credit);
  expect(record.status).toBe("reversed");
  expect(record.decision).toBeTruthy();
  expect(record.reversal_decision).toBeTruthy();
  const receiveNegative = await page.request.post("/api/session", {
    headers: { Origin: origin },
    data: { token: "freight-test-integration-credential-at-least-32-chars" },
  });
  expect(receiveNegative.ok()).toBe(true);
  const negative = await post("events", {
    event_id: `negative-credit-${stamp}`,
    event_type: "edi.freight_invoice.received",
    occurred_at: new Date().toISOString(),
    payload: {
      shipment_id: tender.shipment_reference,
      partner_id: "brex",
      carrier_scac: "BREX",
      invoice_number: `NEG-CM-${stamp}`,
      net_amount: "-0.01",
      currency: "USD",
      invoice_date: today,
    },
  });
  expect(negative.finance_review).toBe(true);
  const verifyCredit = await page.request.post("/api/session", {
    headers: { Origin: origin },
    data: { token: "freight-test-finance-credential-at-least-32-chars" },
  });
  expect(verifyCredit.ok()).toBe(true);
  await page.reload();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /Load board/ })
    .click();
  await page.getByRole("button", { name: reference, exact: true }).click();
  await panel
    .getByRole("button", { name: "Classify carrier invoice", exact: true })
    .click();
  await expect(form.getByLabel("Carrier document treatment")).toHaveValue("");
  await expect(form.getByLabel("Carrier document amount (USD)")).toHaveValue(
    "0.01",
  );
  await form
    .getByLabel("Carrier document treatment")
    .selectOption("credit_memo");
  await form
    .getByLabel("Carrier adjustment description")
    .fill("Carrier explicitly confirmed this negative 210 is a credit memo");
  await form
    .getByRole("button", { name: "Save adjustment draft", exact: true })
    .click();
  await expect(form).not.toBeVisible();
  await confirm(
    panel.getByRole("button", {
      name: "Approve and post adjustment",
      exact: true,
    }),
    "Carrier accounts verified the negative document designation and the exact one-cent credit.",
  );
  w = await snapshot();
  expect(w.settlements.find((v) => v.id === prepared.id).balance_cents).toBe(
    2500,
  );
  expect(
    w.carrier_documents.find((v) => v.number === `NEG-CM-${stamp}`)
      .amount_cents,
  ).toBe(-1);
});

test("carrier credit applications split across invoices and retain noncash reversal and correction evidence", async ({
  page,
}) => {
  test.setTimeout(90000);
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned synthetic carrier accounts",
  );
  const origin = new URL(page.url()).origin,
    suffix = Date.now(),
    today = new Date().toISOString().slice(0, 10);
  const post = async (path, data) => {
    const response = await page.request.post(`/api/freight/${path}`, {
      headers: { Origin: origin, "Idempotency-Key": randomUUID() },
      data,
    });
    expect(response.ok(), await response.text()).toBe(true);
    return (await response.json()).data;
  };
  const snapshot = async () =>
    (await page.request.get("/api/freight/workspace")).json();
  const customer = await post("customers", {
    name: "Carrier allocation qualification customer",
    contact: "ap@example.test",
    billing_address: "25 Receiver Way, Seattle WA 98101",
  });
  async function prepared(name, carrierId, amount) {
    let load = await post("loads", {
      reference: `E2E-CREDIT-${name}-${suffix}`,
      customer_id: customer.id,
      mode: "FTL",
      equipment: "Dry van",
      commodity: "Credit application fixture",
      weight_lbs: 1000,
      miles: 173,
      buy_cents: amount,
      sell_cents: amount + 10000,
      origin: {
        facility: "Origin",
        city: "Portland",
        state: "OR",
        date: today,
        address_lines: ["15 Dock Street"],
        postal_code: "97201",
        country: "US",
      },
      destination: {
        facility: "Receiver",
        city: "Seattle",
        state: "WA",
        date: today,
        address_lines: ["25 Receiver Way"],
        postal_code: "98101",
        country: "US",
      },
    });
    load = await post(`loads/${load.id}/commands`, {
      action: "tender",
      expected_version: load.version,
      carrier_id: carrierId,
    });
    load = await post(`loads/${load.id}/commands`, {
      action: "accept",
      expected_version: load.version,
    });
    let w = await snapshot();
    const tender = w.tenders.find((v) => v.id === load.tender_id),
      carrier = w.carriers.find((v) => v.id === carrierId);
    await post("events", {
      event_id: randomUUID(),
      event_type: "edi.shipment_status.received",
      occurred_at: new Date().toISOString(),
      payload: {
        shipment_id: tender.shipment_reference,
        partner_id: carrier.partner_id,
        carrier_scac: carrier.scac,
        latest_status: "D1",
        events: [{ status_code: "AF" }, { status_code: "D1" }],
      },
    });
    w = await snapshot();
    load = await post(`loads/${load.id}/commands`, {
      action: "attach_pod",
      expected_version: w.loads.find((v) => v.id === load.id).version,
      pod_reference: `Actual external signed receiver evidence ${name}`,
    });
    await post(`loads/${load.id}/pod/review`, {
      expected_version: load.version,
      decision: "accepted",
      note: "Verified externally held signed POD",
    });
    let invoice = await post("invoices", {
      load_id: load.id,
      number: `CREDIT-INV-${name}-${suffix}`,
      amount_cents: amount,
      currency: "USD",
    });
    invoice = await post(`invoices/${invoice.id}/approve`, {});
    return {
      load,
      invoice,
      settlement: await post(`loads/${load.id}/settlement`, {}),
    };
  }
  const source = await prepared("SOURCE", "ca4", 100001),
    a = await prepared("A", "ca4", 20000),
    b = await prepared("B", "ca4", 10000),
    foreign = await prepared("FOREIGN", "ca1", 15000);
  const payout = async (target, amount, reference) =>
    post("cash", {
      direction: "payout",
      currency: "USD",
      account: "credit-qualification-usd",
      external_reference: reference,
      amount_cents: amount,
      cleared_on: today,
      note: "Matched actual cleared payout",
      allocations: [
        {
          target_id: target.id,
          expected_version: target.version,
          amount_cents: amount,
        },
      ],
    });
  await payout(source.settlement, 100001, `SOURCE-PAID-${suffix}`);
  let w = await snapshot();
  const credit = await post(`loads/${source.load.id}/carrier-adjustments`, {
    expected_load_version: w.loads.find((v) => v.id === source.load.id).version,
    invoice_id: source.invoice.id,
    kind: "credit_memo",
    number: `ACCOUNT-CREDIT-${suffix}`,
    document_amount_cents: 4501,
    issued_on: today,
    description: "Carrier-confirmed service credit",
    evidence_reference: `ORIGINAL-CREDIT-${suffix}`,
  });
  w = await snapshot();
  await post(`carrier-adjustments/${credit.id}/review`, {
    expected_load_version: w.loads.find((v) => v.id === source.load.id).version,
    expected_version: credit.version,
    decision: "approved",
    counterparty_contact: "Carrier credit desk",
    confirmation_reference: `CREDIT-CONF-${suffix}`,
    note: "Inspected actual carrier credit memo",
  });
  const file = await page.request.post(
    `/api/freight/loads/${source.load.id}/documents?kind=other&filename=carrier-application.pdf`,
    {
      headers: {
        Origin: origin,
        "Idempotency-Key": randomUUID(),
        "Content-Type": "application/pdf",
      },
      data: Buffer.from(
        "%PDF-1.4\nactual synthetic carrier allocation confirmation\n%%EOF\n",
      ),
    },
  );
  expect(file.ok()).toBe(true);
  const document = (await file.json()).data;
  async function login(role) {
    const response = await page.request.post("/api/session", {
      headers: { Origin: origin },
      data: { token: `freight-test-${role}-credential-at-least-32-chars` },
    });
    expect(response.ok()).toBe(true);
    await page.goto("/");
    await page
      .getByRole("navigation")
      .getByRole("button", { name: "Freight audit", exact: true })
      .click();
    await page
      .getByRole("tab", { name: "Bank reconciliation", exact: true })
      .click();
  }
  await login("finance");
  const panel = page.getByRole("region", {
      name: "Carrier credit applications",
      exact: true,
    }),
    dialog = page.getByRole("dialog"),
    reference = `CARRIER-APPLICATION-${suffix}`;
  await panel
    .getByRole("button", { name: "Apply carrier credit", exact: true })
    .click();
  await dialog
    .getByLabel("Carrier credit source")
    .selectOption(source.settlement.id);
  await expect(
    dialog.getByRole("checkbox", {
      name: `Apply credit to ${foreign.load.reference}`,
      exact: true,
    }),
  ).toHaveCount(0);
  async function allocate(target, amount) {
    await dialog
      .getByRole("checkbox", {
        name: `Apply credit to ${target.load.reference}`,
        exact: true,
      })
      .check();
    await dialog
      .getByLabel(`Credit amount for ${target.load.reference}`, { exact: true })
      .fill(amount);
  }
  async function confirm(contact, confirmation, note) {
    await dialog.getByLabel("Carrier application contact").fill(contact);
    await dialog
      .getByLabel("Carrier application confirmation")
      .fill(confirmation);
    await dialog.getByLabel("Carrier application decision note").fill(note);
  }
  await allocate(a, "12.51");
  await allocate(b, "32.50");
  await dialog
    .getByLabel("Carrier credit application reference")
    .fill(reference);
  await dialog
    .getByLabel("Stored application evidence (optional)")
    .selectOption(document.id);
  await confirm(
    "Carrier AR representative",
    `APPLICATION-CONF-${suffix}`,
    "Carrier verified the two invoice allocations",
  );
  await dialog
    .getByRole("button", {
      name: "Confirm carrier credit application",
      exact: true,
    })
    .click();
  await expect(dialog).not.toBeVisible();
  let card = panel
    .locator("article")
    .filter({ has: page.getByText(reference, { exact: true }) });
  await expect(card.getByText("Applied", { exact: true })).toBeVisible();
  w = await snapshot();
  const balance = (id) => w.settlements.find((v) => v.id === id).balance_cents;
  expect(balance(source.settlement.id)).toBe(0);
  expect(balance(a.settlement.id)).toBe(18749);
  expect(balance(b.settlement.id)).toBe(6750);
  const expenseBefore = w.finance.accounts.find(
    (v) => v.account === "freight_expense",
  ).net_debit_cents;
  const original = w.carrier_credit_applications.find(
    (v) => v.evidence_reference === reference,
  );
  expect(
    w.cash_entries.filter((v) =>
      v.allocations.some((x) =>
        [source.load.id, a.load.id, b.load.id].includes(x.load_id),
      ),
    ),
  ).toHaveLength(1);
  await payout(
    w.settlements.find((v) => v.id === a.settlement.id),
    18749,
    `TARGET-A-PAID-${suffix}`,
  );
  w = await snapshot();
  await payout(
    w.settlements.find((v) => v.id === b.settlement.id),
    6750,
    `TARGET-B-PAID-${suffix}`,
  );
  await page.reload();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Freight audit", exact: true })
    .click();
  await page
    .getByRole("tab", { name: "Bank reconciliation", exact: true })
    .click();
  await card
    .getByRole("button", { name: "Reverse credit application", exact: true })
    .click();
  await confirm(
    "Carrier corrections desk",
    `RETRACTION-${suffix}`,
    "Carrier retracted the original allocation split",
  );
  await dialog
    .getByRole("button", { name: "Confirm application reversal", exact: true })
    .click();
  await expect(dialog).not.toBeVisible();
  w = await snapshot();
  expect(balance(source.settlement.id)).toBe(-4501);
  expect(balance(a.settlement.id)).toBe(1251);
  expect(balance(b.settlement.id)).toBe(3250);
  await card
    .getByRole("button", { name: "Correct reversed application", exact: true })
    .click();
  await expect(
    dialog.getByLabel("Carrier credit application reference"),
  ).toHaveAttribute("readonly", "");
  await allocate(a, "12.51");
  await allocate(b, "17.50");
  await confirm(
    "Carrier corrections desk",
    `CORRECTION-${suffix}`,
    "Carrier confirmed corrected partial allocations",
  );
  await dialog
    .getByRole("button", {
      name: "Confirm carrier credit application",
      exact: true,
    })
    .click();
  await expect(dialog).not.toBeVisible();
  card = panel
    .locator("article")
    .filter({ has: page.getByText(reference, { exact: true }) });
  const applied = card.filter({
      has: page.getByText("Applied", { exact: true }),
    }),
    reversed = card.filter({
      has: page.getByText("Reversed", { exact: true }),
    });
  await applied
    .getByText("Application evidence and history", { exact: true })
    .click();
  await reversed
    .getByText("Application evidence and history", { exact: true })
    .click();
  await expect(
    reversed.getByText(
      `Carrier AR representative · APPLICATION-CONF-${suffix}`,
      { exact: true },
    ),
  ).toBeVisible();
  await expect(
    reversed.getByText(`Carrier corrections desk · RETRACTION-${suffix}`, {
      exact: true,
    }),
  ).toBeVisible();
  await expect(
    applied.getByText(`Carrier corrections desk · CORRECTION-${suffix}`, {
      exact: true,
    }),
  ).toBeVisible();
  w = await snapshot();
  expect(balance(source.settlement.id)).toBe(-1500);
  expect(balance(a.settlement.id)).toBe(0);
  expect(balance(b.settlement.id)).toBe(1500);
  expect(
    w.finance.accounts.find((v) => v.account === "freight_expense")
      .net_debit_cents,
  ).toBe(expenseBefore);
  const { approval_control: sourceControl, ...retainedSourceInvoice } =
    w.invoices.find((v) => v.id === source.invoice.id);
  expect(sourceControl.state).toBe("approved");
  expect(retainedSourceInvoice).toEqual(source.invoice);
  const corrected = w.carrier_credit_applications.find(
    (v) => v.corrects_id === original.id,
  );
  expect(corrected.amount_cents).toBe(3001);
  expect(
    w.carrier_credit_applications.find((v) => v.id === original.id)
      .corrected_by_id,
  ).toBe(corrected.id);
  expect(
    w.cash_entries.filter((v) =>
      v.allocations.some((x) =>
        [source.load.id, a.load.id, b.load.id].includes(x.load_id),
      ),
    ),
  ).toHaveLength(3);
  if (process.env.FREIGHT_E2E_SCREENSHOTS === "true")
    await panel.screenshot({
      path: "../docs/screenshots/carrier-credit-applications.png",
    });
  await page.setViewportSize({ width: 390, height: 844 });
  await panel
    .getByRole("button", { name: "Apply carrier credit", exact: true })
    .click();
  await dialog
    .getByLabel("Carrier credit source")
    .selectOption(source.settlement.id);
  await allocate(b, "15.00");
  expect(
    await dialog.evaluate((el) => el.getBoundingClientRect().width),
  ).toBeLessThan(400);
  await page.keyboard.press("Escape");
  await expect(dialog).not.toBeVisible();
  await page.setViewportSize({ width: 1280, height: 900 });
  await login("viewer");
  await expect(
    panel.getByRole("button", { name: "Apply carrier credit", exact: true }),
  ).toBeDisabled();
  await expect(
    panel.getByRole("button", {
      name: "Reverse credit application",
      exact: true,
    }),
  ).toBeDisabled();
  const denied = await page.request.post(
    "/api/freight/carrier-credit-applications",
    { headers: { Origin: origin, "Idempotency-Key": randomUUID() }, data: {} },
  );
  expect(denied.status()).toBe(403);
});

test("mobile navigation, filtering, modal keyboard behavior, and CSRF rejection", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/");
  await expect(
    page.getByRole("heading", { name: "Overview", exact: true }),
  ).toBeVisible();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBe(true);
  await page.getByRole("button", { name: "Toggle navigation" }).click();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /Load board/ })
    .click();
  await page.getByLabel("Search loads").fill("Portland");
  await expect(
    page.getByRole("button", { name: "FR-1048", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "FR-1050", exact: true }),
  ).toHaveCount(0);
  await page.getByRole("button", { name: "Create load", exact: true }).click();
  await expect(page.getByRole("dialog")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog")).not.toBeVisible();
  const response = await page.request.post("/api/freight/customers", {
    headers: { Origin: "https://untrusted.example" },
    data: { name: "CSRF", contact: "" },
  });
  expect(response.status()).toBe(403);
  if (process.env.FREIGHT_E2E_SCREENSHOTS === "true")
    await page.screenshot({
      path: "../docs/screenshots/mobile.png",
      fullPage: true,
    });
});

test("accepted quote preserves an imported Sync shipment and holds cancellation tracking that would mark its order shipped", async ({
  page,
}, testInfo) => {
  test.setTimeout(90000);
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned canonical Sync fixture",
  );
  const origin = new URL(page.url()).origin,
    stamp = Date.now(),
    reference = "SOURCE-E2E-" + stamp,
    quoteRef = "QUOTE-SOURCE-" + stamp,
    date = new Date(Date.now() + 2 * 86400000).toISOString().slice(0, 10);
  const fixtureHeaders = { "x-fixture-key": "freight-sync-fixture-only" };
  const order = {
    id: randomUUID(),
    tenantId: "00000000-0000-4000-8000-000000000901",
    orderNumber: "SOURCE-ORDER-" + stamp,
    status: "pending",
    grandTotal: 2345,
    metadata: { source: "Immutable commerce snapshot" },
  };
  const seeded = await page.request.put(
    `${fixtureOrigin}/__sync_fixture/order`,
    { headers: fixtureHeaders, data: order },
  );
  expect(seeded.ok()).toBe(true);
  const session = await page.request.post("/api/session", {
    headers: { Origin: origin },
    data: { token: "freight-test-quote-source-credential-at-least-32-chars" },
  });
  expect(session.ok()).toBe(true);
  const post = async (path, data) => {
    const r = await page.request.post("/api/freight/" + path, {
      headers: { Origin: origin, "Idempotency-Key": randomUUID() },
      data,
    });
    expect(r.ok(), await r.text()).toBe(true);
    return (await r.json()).data;
  };
  const snapshot = async () => {
    const r = await page.request.get("/api/freight/workspace");
    expect(r.ok()).toBe(true);
    return r.json();
  };
  const customer = await post("customers", {
    name: "Owned source customer",
    contact: "quote-source@example.test",
    billing_address: "1 Synthetic Customer Way",
  });
  const carrier = await post("carriers", {
    name: "Owned source carrier",
    scac: "QSRC",
    mc_number: "MC-TEST-901",
    insurance_expires: new Date(Date.now() + 365 * 86400000)
      .toISOString()
      .slice(0, 10),
    equipment: "Dry van",
    partner_id: "quote-source-carrier",
  });
  await post("finance/profile", {
    expected_version: 0,
    legal_name: "Owned quote-source shipper",
    address: "2 Synthetic Issuer Street",
    contact: "billing@example.test",
    payment_instructions: "Synthetic external bank instructions",
  });
  const load = await post("sync/import", {
    order_id: order.id,
    load: {
      reference,
      customer_id: customer.id,
      mode: "FTL",
      equipment: "Dry van",
      commodity: "Original commerce goods",
      weight_lbs: 1000,
      miles: 173,
      buy_cents: 90000,
      sell_cents: 140000,
      origin: {
        facility: "Source Portland dock",
        city: "Portland",
        state: "OR",
        date,
        address_lines: ["15 Dock Street", "Gate 2"],
        postal_code: "97201",
        country: "US",
      },
      destination: {
        facility: "Source Seattle dock",
        city: "Seattle",
        state: "WA",
        date,
        address_lines: ["25 Receiving Ave"],
        postal_code: "98101",
        country: "US",
      },
    },
  });
  expect(load.source_order).toEqual(order);
  await page.reload();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Quotes", exact: true })
    .click();
  await page.getByRole("button", { name: "Create quote", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Quote source shipment").selectOption(load.id);
  await expect(
    dialog.getByLabel("Quoted customer", { exact: true }),
  ).toBeDisabled();
  await expect(dialog.getByLabel("Quote pickup street")).toHaveValue(
    "15 Dock Street\nGate 2",
  );
  await dialog.getByLabel("Quote reference", { exact: true }).fill(quoteRef);
  await dialog.getByLabel("Customer quote price").fill("1500.01");
  await dialog.getByRole("button", { name: "Save quote action" }).click();
  await expect(dialog).not.toBeVisible();
  const detail = page.getByRole("region", {
    name: "Quote details " + quoteRef,
  });
  await expect(detail).toContainText(order.orderNumber);
  const proof = async (ref) => {
    await dialog
      .getByLabel("Quote counterparty contact")
      .fill("Actual source counterparty");
    await dialog.getByLabel("Quote confirmation reference").fill(ref);
    await dialog
      .getByLabel("Quote evidence note")
      .fill("Verified original source correspondence");
  };
  await detail
    .getByRole("button", { name: "Record carrier offer", exact: true })
    .click();
  await dialog.getByLabel("Quoted carrier").selectOption(carrier.id);
  await dialog.getByLabel("Actual carrier offer (USD)").fill("1250.01");
  await proof("SOURCE-CARRIER-RATE");
  await dialog.getByRole("button", { name: "Save quote action" }).click();
  await expect(dialog).not.toBeVisible();
  let w = await snapshot(),
    q = w.quotes.find((v) => v.reference === quoteRef);
  await detail.getByRole("button", { name: "Issue customer quote" }).click();
  await dialog
    .getByLabel("Selected carrier offer")
    .selectOption(q.revisions[0].offers[0].id);
  await dialog.getByRole("button", { name: "Save quote action" }).click();
  await expect(dialog).not.toBeVisible();
  await detail
    .getByRole("button", { name: "Record customer acceptance" })
    .click();
  await proof("SOURCE-CUSTOMER-ACCEPTED");
  await dialog.getByRole("button", { name: "Save quote action" }).click();
  await expect(dialog).not.toBeVisible();
  await detail.getByRole("button", { name: "Book quoted shipment" }).click();
  await dialog.getByRole("button", { name: "Save quote action" }).click();
  await expect(dialog).not.toBeVisible();
  w = await snapshot();
  q = w.quotes.find((v) => v.reference === quoteRef);
  expect(w.loads).toHaveLength(1);
  expect(q.booked_load_id).toBe(load.id);
  expect(q.booking_source.buy_cents).toBe(90000);
  expect(w.loads[0]).toMatchObject({
    id: load.id,
    reference,
    buy_cents: 125001,
    sell_cents: 150001,
    sync_order_id: order.id,
    order_number: order.orderNumber,
    source_order: order,
  });
  expect(w.loads[0].origin.address_lines).toEqual(["15 Dock Street", "Gate 2"]);
  expect(w.journal).toHaveLength(0);
  expect(w.outbox).toHaveLength(0);
  const document = await page.request.get(
    "/api/freight/quotes/" + q.id + "/statement",
  );
  expect(document.ok()).toBe(true);
  expect(await document.text()).toContain(order.orderNumber);
  const captures = async () => {
    const r = await page.request.get(
      `${fixtureOrigin}/__sync_fixture/tracking`,
      { headers: fixtureHeaders },
    );
    expect(r.ok()).toBe(true);
    return (await r.json()).tracking;
  };
  expect(await captures()).toHaveLength(0);
  if (!testInfo.retry) {
    await page.setViewportSize({ width: 1280, height: 1000 });
    await detail.screenshot({ path: "../docs/screenshots/quote-source.png" });
  }
  await post("loads/" + load.id + "/commands", {
    action: "cancel",
    expected_version: w.loads[0].version,
  });
  await expect
    .poll(
      async () =>
        (await snapshot()).outbox.find((v) => v.load_id === load.id)?.status,
      { timeout: 15000 },
    )
    .toBe("held");
  expect(await captures()).toHaveLength(0);
  const captured = await page.request.get(
    `${fixtureOrigin}/__sync_fixture/tracking`,
    { headers: fixtureHeaders },
  );
  expect((await captured.json()).order.status).toBe("pending");
  expect((await snapshot()).loads[0].source_order).toEqual(order);
});

test("source cancellation requires explicit customer review and safe withdrawal while retaining canonical and freight history", async ({
  page,
}) => {
  test.setTimeout(90000);
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned Sync source fixture",
  );
  const origin = new URL(page.url()).origin,
    stamp = Date.now(),
    reference = `SOURCE-REVIEW-${stamp}`,
    fixtureHeaders = { "x-fixture-key": "freight-sync-fixture-only" },
    order = {
      id: randomUUID(),
      orderNumber: `REVIEW-PO-${stamp}`,
      status: "pending",
      lineItems: [{ sku: "ORIGINAL-SKU", quantity: 2 }],
      metadata: { original: "Retain exact raw import" },
    };
  const control = async (method, path, data) => {
    const result = await page.request[method](
      `${fixtureOrigin}/__sync_fixture/` + path,
      { headers: fixtureHeaders, data },
    );
    expect(result.ok()).toBe(true);
    return result;
  };
  await control("put", "order", order);
  await page.request.post("/api/session", {
    headers: { Origin: origin },
    data: { token: "freight-test-quote-source-credential-at-least-32-chars" },
  });
  const snapshot = async () =>
    (await page.request.get("/api/freight/workspace")).json();
  const post = async (path, data, expected = 200) => {
    const result = await page.request.post("/api/freight/" + path, {
      headers: { Origin: origin, "Idempotency-Key": randomUUID() },
      data,
    });
    expect(result.status(), await result.text()).toBe(expected);
    return (await result.json()).data;
  };
  const customer = await post("customers", {
    name: "Source review customer",
    contact: "owned@example.test",
  });
  const carrier = await post("carriers", {
    name: "Source review carrier",
    scac: "SRVW",
    mc_number: "MC-OWNED-REVIEW",
    insurance_expires: new Date(Date.now() + 365 * 86400000)
      .toISOString()
      .slice(0, 10),
    equipment: "Dry van",
    partner_id: "source-review-owned",
  });
  const day = new Date(Date.now() + 2 * 86400000).toISOString().slice(0, 10);
  const load = await post("sync/import", {
    order_id: order.id,
    load: {
      reference,
      customer_id: customer.id,
      mode: "FTL",
      equipment: "Dry van",
      commodity: "Original goods",
      weight_lbs: 1000,
      miles: 173,
      buy_cents: 123456,
      sell_cents: 150001,
      origin: {
        facility: "Source dock",
        city: "Portland",
        state: "OR",
        date: day,
        address_lines: ["15 Dock Street"],
        postal_code: "97201",
        country: "US",
      },
      destination: {
        facility: "Destination dock",
        city: "Seattle",
        state: "WA",
        date: day,
        address_lines: ["25 Receiving Ave"],
        postal_code: "98101",
        country: "US",
      },
    },
  });
  await control("patch", "order", { status: "cancelled" });
  await post(
    `loads/${load.id}/commands`,
    {
      action: "tender",
      carrier_id: carrier.id,
      expected_version: load.version,
    },
    409,
  );
  let w = await snapshot();
  expect(w.loads.find((v) => v.id === load.id).source_order).toEqual(order);
  expect(w.outbox.filter((v) => v.load_id === load.id)).toHaveLength(0);
  await page.reload();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Integrations", exact: true })
    .click();
  const queue = page.getByRole("region", {
    name: "Commerce source exceptions",
  });
  await queue.getByRole("button").filter({ hasText: reference }).click();
  const dialog = page.getByRole("dialog"),
    source = dialog.getByRole("region", { name: "Commerce source control" });
  await expect(source).toContainText("latest status cancelled");
  await expect(
    dialog.getByRole("button", { name: "Tender", exact: true }),
  ).toBeDisabled();
  await source
    .getByRole("button", { name: "Record customer source decision" })
    .click();
  const form = source.getByRole("form", { name: "Customer source decision" });
  await expect(form.getByLabel("Source decision")).toHaveValue("");
  await form.getByLabel("Source decision").selectOption("authorize_transport");
  await form
    .getByLabel("Actual customer contact")
    .fill("Verified customer transport lead");
  await form
    .getByLabel("Customer source confirmation reference")
    .fill("CUSTOMER-EXPLICIT-CONSENT");
  await form
    .getByLabel("Source review findings")
    .fill(
      "Customer explicitly requests current transport despite cancelled commerce order",
    );
  if (process.env.FREIGHT_E2E_SCREENSHOTS === "true")
    await form.screenshot({
      path: "../docs/screenshots/source-review-pending.png",
    });
  await form.getByRole("button", { name: "Save source decision" }).click();
  await expect(form).not.toBeVisible();
  await expect(
    dialog.getByRole("button", { name: "Tender", exact: true }),
  ).toBeEnabled();
  w = await snapshot();
  await post(`loads/${load.id}/commands`, {
    action: "tender",
    carrier_id: carrier.id,
    expected_version: w.loads.find((v) => v.id === load.id).version,
  });
  await control("patch", "order", {
    lineItems: [{ sku: "ORIGINAL-SKU", quantity: 9 }],
  });
  await source.getByRole("button", { name: "Refresh commerce order" }).click();
  await expect(source).toContainText("Source change requires review");
  w = await snapshot();
  expect(w.outbox.find((v) => v.load_id === load.id).transmission).toBe(
    "not_started",
  );
  await dialog
    .getByRole("button", { name: "Withdraw unsent tender", exact: true })
    .click();
  await dialog
    .getByRole("button", { name: "Confirm withdrawal", exact: true })
    .click();
  await expect(
    dialog.getByRole("button", { name: "Cancel", exact: true }),
  ).toBeVisible();
  w = await snapshot();
  await post(`loads/${load.id}/commands`, {
    action: "cancel",
    expected_version: w.loads.find((v) => v.id === load.id).version,
  });
  await source.getByRole("button", { name: "Refresh commerce order" }).click();
  await source
    .getByRole("button", { name: "Record customer source decision" })
    .click();
  await form.getByLabel("Source decision").selectOption("cancel_transport");
  await form
    .getByLabel("Actual customer contact")
    .fill("Verified customer transport lead");
  await form
    .getByLabel("Customer source confirmation reference")
    .fill("CUSTOMER-SAFE-CANCEL");
  await form
    .getByLabel("Source review findings")
    .fill("Proven unsent tender withdrawn and freight cancelled safely");
  await form.getByRole("button", { name: "Save source decision" }).click();
  await expect(form).not.toBeVisible();
  await expect(source).toContainText("Source decision history (2)");
  await expect
    .poll(
      async () =>
        (await snapshot()).outbox.find(
          (v) =>
            v.load_id === load.id &&
            v.destination === "stateset-sync-server" &&
            v.payload.trackingData.freight_status === "cancelled",
        )?.status,
      { timeout: 15000 },
    )
    .toBe("held");
  w = await snapshot();
  expect(
    w.outbox
      .filter(
        (v) =>
          v.load_id === load.id && v.destination === "stateset-sync-server",
      )
      .every((v) => v.total_attempts === 0),
  ).toBe(true);
  expect(w.loads.find((v) => v.id === load.id)).toMatchObject({
    status: "cancelled",
    source_order: order,
    buy_cents: 123456,
    sell_cents: 150001,
  });
  expect(
    w.source_reviews
      .filter((v) => v.load_id === load.id)
      .map((v) => v.decision.decision),
  ).toEqual(["authorize_transport", "cancel_transport"]);
  const captured = await control("get", "tracking");
  const upstream = await captured.json();
  expect(upstream.order.status).toBe("cancelled");
  expect(upstream.tracking).toHaveLength(0);
  await page.setViewportSize({ width: 390, height: 844 });
  expect(await dialog.evaluate((el) => el.scrollWidth <= el.clientWidth)).toBe(
    true,
  );
  if (process.env.FREIGHT_E2E_SCREENSHOTS === "true")
    await source.screenshot({ path: "../docs/screenshots/source-review.png" });
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.request.post("/api/session", {
    headers: { Origin: origin },
    data: { token: "freight-test-source-viewer-credential-at-least-32-chars" },
  });
  await page.reload();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /Load board/ })
    .click();
  await page.getByLabel("Search loads").fill(reference);
  await page.getByRole("button", { name: reference, exact: true }).click();
  await expect(
    source.getByRole("button", { name: "Refresh commerce order" }),
  ).toBeDisabled();
  await post(`loads/${load.id}/source/refresh`, {}, 403);
});

async function customerAmendmentFixture(page, imported, partnerScoped = false) {
  const origin = new URL(page.url()).origin,
    stamp = Date.now(),
    reference = `AMEND-BASE-${stamp}`;
  if (imported && !partnerScoped) {
    const login = await page.request.post("/api/session", {
      headers: { Origin: origin },
      data: { token: "freight-test-quote-source-credential-at-least-32-chars" },
    });
    expect(login.ok()).toBe(true);
  }
  const post = async (path, data, expected = 200) => {
    const result = await page.request.post("/api/freight/" + path, {
      headers: { Origin: origin, "Idempotency-Key": randomUUID() },
      data,
    });
    expect(result.status(), await result.text()).toBe(expected);
    return (await result.json()).data;
  };
  const snapshot = async () => {
    const r = await page.request.get("/api/freight/workspace");
    expect(r.ok()).toBe(true);
    return r.json();
  };
  let customer = "c1",
    originalCarrier = "ca4",
    replacementCarrier = "ca1",
    order = null;
  const fixtureHeaders = { "x-fixture-key": "freight-sync-fixture-only" };
  if (imported) {
    customer = partnerScoped
      ? "c1"
      : (
          await post("customers", {
            name: "Amendment source customer",
            contact: "owned-amendment@example.test",
            billing_address: "1 Actual Synthetic Billing Street",
          })
        ).id;
    const carrier = async (name, scac) =>
      post("carriers", {
        name,
        scac,
        mc_number: "MC-OWNED-AMENDMENT",
        insurance_expires: new Date(Date.now() + 365 * 86400000)
          .toISOString()
          .slice(0, 10),
        equipment: "Dry van",
        partner_id: `amend-${scac.toLowerCase()}`,
      });
    originalCarrier = (await carrier("Original amendment carrier", "AMAA")).id;
    replacementCarrier = (
      await carrier("Replacement amendment carrier", "AMAB")
    ).id;
    if (!(await snapshot()).billing_profile)
      await post("finance/profile", {
        expected_version: 0,
        legal_name: "Owned source amendment shipper",
        address: "2 Synthetic Issuer Street",
        contact: "billing@example.test",
        payment_instructions: "Synthetic external bank instructions",
      });
    order = {
      id: randomUUID(),
      orderNumber: `AMEND-SOURCE-${stamp}`,
      status: "pending",
      lineItems: [{ sku: "ORIGINAL-GOODS", quantity: 2 }],
      metadata: { original: "Keep exact source evidence" },
    };
    const seed = await page.request.put(
      `${fixtureOrigin}/__sync_fixture/order`,
      { headers: fixtureHeaders, data: order },
    );
    expect(seed.ok()).toBe(true);
  }
  const day = new Date(Date.now() + 2 * 86400000).toISOString().slice(0, 10),
    plan = {
      customer_id: customer,
      mode: "FTL",
      equipment: "Dry van",
      commodity: "Amendment original freight",
      weight_lbs: 1000,
      miles: 173,
      origin: {
        facility: "Original Portland dock",
        city: "Portland",
        state: "OR",
        date: day,
        address_lines: ["15 Dock Street", "Gate 2"],
        postal_code: "97201",
        country: "US",
        contact: "Original pickup lead",
        phone: "555-0101",
        instructions: "Preserve the original pickup instructions",
      },
      destination: {
        facility: "Original Seattle dock",
        city: "Seattle",
        state: "WA",
        date: day,
        address_lines: ["25 Receiving Ave"],
        postal_code: "98101",
        country: "US",
        contact: "Original receiving lead",
      },
    };
  let source = null;
  if (imported)
    source = await post("sync/import", {
      order_id: order.id,
      load: {
        ...plan,
        reference: reference + "-LOAD",
        buy_cents: 90000,
        sell_cents: 140000,
      },
    });
  let q = await post("quotes", {
    reference,
    plan,
    sell_cents: 150001,
    valid_until: new Date(Date.now() + 12 * 3600000).toISOString(),
    terms: "Original inclusive customer agreement",
    ...(source
      ? { planned_load_id: source.id, expected_load_version: source.version }
      : {}),
  });
  q = await post(`quotes/${q.id}/offers`, {
    expected_version: q.version,
    carrier_id: originalCarrier,
    amount_cents: 125001,
    valid_until: new Date(Date.now() + 24 * 3600000).toISOString(),
    contact: "Original carrier dispatch",
    reference: "ORIGINAL-AMENDMENT-PRIVATE-OFFER",
    note: "Actual inclusive original carrier quote",
  });
  q = await post(`quotes/${q.id}/issue`, {
    expected_version: q.version,
    offer_id: q.revisions[0].offers[0].id,
  });
  q = await post(`quotes/${q.id}/decision`, {
    expected_version: q.version,
    decision: "accept",
    contact: "Original customer transport lead",
    reference: "ORIGINAL-CUSTOMER-AGREEMENT",
    note: "Original accepted shipment plan and price",
  });
  q = await post(`quotes/${q.id}/book`, {
    expected_version: q.version,
    ...(source ? { expected_load_version: source.version } : {}),
  });
  return {
    origin,
    reference,
    post,
    snapshot,
    quote: q,
    loadId: q.booked_load_id,
    originalCarrier,
    replacementCarrier,
    order,
    fixtureHeaders,
  };
}
async function proposeAmendmentInBrowser(page, fixture) {
  await page.reload();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Quotes", exact: true })
    .click();
  await page
    .getByRole("button", {
      name: `Open quote ${fixture.reference}`,
      exact: true,
    })
    .click();
  const original = page.getByRole("region", {
    name: `Quote details ${fixture.reference}`,
  });
  await original
    .getByRole("button", { name: "Propose customer amendment", exact: true })
    .click();
  const dialog = page.getByRole("dialog"),
    reference = fixture.reference + "-A1";
  await expect(
    dialog.getByLabel("Quoted customer", { exact: true }),
  ).toBeDisabled();
  await expect(dialog.getByLabel("Quote pickup street")).toHaveValue(
    "15 Dock Street\nGate 2",
  );
  await expect(dialog.getByLabel("Quote pickup instructions")).toHaveValue(
    "Preserve the original pickup instructions",
  );
  await dialog.getByLabel("Quote reference", { exact: true }).fill(reference);
  await dialog
    .getByLabel("Quote revision reason")
    .fill(
      "Actual customer asks for revised Tacoma receiving dock and inclusive price",
    );
  await dialog
    .getByLabel("Quote delivery facility")
    .fill("Amended Tacoma dock");
  await dialog.getByLabel("Quote delivery city").fill("Tacoma");
  await dialog
    .getByLabel("Quote delivery street")
    .fill("99 Amended Dock\nGate B");
  await dialog.getByLabel("Quote delivery postal code").fill("98401");
  await dialog
    .getByLabel("Quote delivery contact")
    .fill("Verified Tacoma receiving lead");
  await dialog.getByLabel("Customer quote price").fill("1700.01");
  await dialog.getByRole("button", { name: "Save quote action" }).click();
  await expect(dialog).not.toBeVisible();
  const amended = page.getByRole("region", {
    name: `Quote details ${reference}`,
  });
  await expect(amended).toContainText(
    `Customer amendment of ${fixture.reference}`,
  );
  let w = await fixture.snapshot();
  expect(w.loads.find((v) => v.id === fixture.loadId)).toMatchObject({
    sell_cents: 150001,
    buy_cents: 125001,
    destination: { city: "Seattle" },
    quote_binding: { id: fixture.quote.id },
  });
  await amended
    .getByRole("button", { name: "Record carrier offer", exact: true })
    .click();
  await dialog
    .getByLabel("Quoted carrier")
    .selectOption(fixture.replacementCarrier);
  await dialog.getByLabel("Actual carrier offer (USD)").fill("1400.01");
  await dialog
    .getByLabel("Quote counterparty contact")
    .fill("Actual replacement carrier dispatch");
  await dialog
    .getByLabel("Quote confirmation reference")
    .fill("PRIVATE-AMENDED-CARRIER-OFFER");
  await dialog
    .getByLabel("Quote evidence note")
    .fill("Actual inclusive offer for amended plan");
  await dialog.getByRole("button", { name: "Save quote action" }).click();
  await expect(dialog).not.toBeVisible();
  w = await fixture.snapshot();
  let q = w.quotes.find((v) => v.reference === reference);
  await amended.getByRole("button", { name: "Issue customer quote" }).click();
  await dialog
    .getByLabel("Selected carrier offer")
    .selectOption(q.revisions[0].offers[0].id);
  await dialog.getByRole("button", { name: "Save quote action" }).click();
  await expect(dialog).not.toBeVisible();
  return { reference, amended, dialog, quoteId: q.id };
}
async function acceptAmendmentInBrowser(
  page,
  amendment,
  explicitSource = false,
) {
  await amendment.amended
    .getByRole("button", { name: "Record customer acceptance", exact: true })
    .click();
  await amendment.dialog
    .getByLabel("Quote counterparty contact")
    .fill("Verified customer transport lead");
  await amendment.dialog
    .getByLabel("Quote confirmation reference")
    .fill("ACTUAL-AMENDED-CUSTOMER-CONSENT");
  await amendment.dialog
    .getByLabel("Quote evidence note")
    .fill("Customer confirms this amended plan and all source findings");
  if (explicitSource) {
    await expect(amendment.dialog).toContainText("status cancelled");
    const consent = amendment.dialog.getByRole("checkbox", {
      name: /Customer explicitly authorizes this amended transport/,
    });
    await expect(consent).not.toBeChecked();
    await consent.check();
    if (process.env.FREIGHT_E2E_SCREENSHOTS === "true")
      await amendment.dialog
        .getByRole("button", { name: "Save quote action" })
        .scrollIntoViewIfNeeded();
  }
  await amendment.dialog
    .getByRole("button", { name: "Save quote action" })
    .click();
  await expect(amendment.dialog).not.toBeVisible();
}
test("customer amendment retains current contract until safe carrier withdrawal and activates the revised plan on the same shipment", async ({
  page,
}) => {
  test.setTimeout(90000);
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned amendment workspace",
  );
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  const f = await customerAmendmentFixture(page, false);
  let w = await f.snapshot();
  await f.post(`loads/${f.loadId}/commands`, {
    action: "tender",
    carrier_id: f.originalCarrier,
    expected_version: w.loads.find((v) => v.id === f.loadId).version,
  });
  const a = await proposeAmendmentInBrowser(page, f);
  await acceptAmendmentInBrowser(page, a);
  await a.amended
    .getByRole("button", { name: "Activate customer amendment" })
    .click();
  await a.dialog.getByRole("button", { name: "Save quote action" }).click();
  await expect(a.dialog.getByRole("alert")).toContainText("Safely withdraw");
  await page.keyboard.press("Escape");
  w = await f.snapshot();
  expect(w.loads.find((v) => v.id === f.loadId)).toMatchObject({
    status: "tendered",
    sell_cents: 150001,
    destination: { city: "Seattle" },
  });
  await a.amended.getByRole("button", { name: "Open source shipment" }).click();
  let detail = page.getByRole("dialog");
  await detail
    .getByRole("button", { name: "Withdraw unsent tender", exact: true })
    .click();
  await detail
    .getByRole("button", { name: "Confirm withdrawal", exact: true })
    .click();
  await expect(
    detail.getByRole("button", { name: "Tender", exact: true }),
  ).toBeVisible();
  await page.keyboard.press("Escape");
  await a.amended
    .getByRole("button", { name: "Activate customer amendment" })
    .click();
  await a.dialog.getByRole("button", { name: "Save quote action" }).click();
  await expect(a.dialog).not.toBeVisible();
  w = await f.snapshot();
  const l = w.loads.find((v) => v.id === f.loadId);
  expect(l).toMatchObject({
    reference: f.reference,
    status: "planned",
    buy_cents: 140001,
    sell_cents: 170001,
    quote_binding: { id: a.quoteId },
    destination: {
      city: "Tacoma",
      address_lines: ["99 Amended Dock", "Gate B"],
      contact: "Verified Tacoma receiving lead",
    },
    origin: {
      address_lines: ["15 Dock Street", "Gate 2"],
      instructions: "Preserve the original pickup instructions",
    },
  });
  expect(w.quotes.find((q) => q.id === f.quote.id)).toMatchObject({
    status: "superseded",
    revisions: [
      { sell_cents: 150001, plan: { destination: { city: "Seattle" } } },
    ],
    superseded_by: { quote_id: a.quoteId },
  });
  const doc = await page.request.get(
      `/api/freight/quotes/${a.quoteId}/statement`,
    ),
    html = await doc.text();
  expect(doc.ok()).toBe(true);
  expect(html).toContain("1700.01");
  expect(html).toContain("Customer amendment of quote");
  expect(html).not.toContain("1400.01");
  expect(html).not.toContain("PRIVATE-AMENDED-CARRIER-OFFER");
  await a.amended
    .getByRole("button", { name: "View original agreement" })
    .click();
  const old = page.getByRole("region", {
    name: `Quote details ${f.reference}`,
  });
  await expect(old).toContainText(`Replaced by ${a.reference}`);
  await expect(
    old.getByRole("button", { name: "Propose customer amendment" }),
  ).toHaveCount(0);
  await old.getByRole("button", { name: "View replacement agreement" }).click();
  await page.setViewportSize({ width: 390, height: 844 });
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBe(true);
  if (process.env.FREIGHT_E2E_SCREENSHOTS === "true")
    await a.amended
      .locator(".amendment-context")
      .screenshot({ path: "../docs/screenshots/customer-amendment.png" });
  expect(errors).toEqual([]);
});
test("imported customer amendment requires explicit canonical source consent and retains the commerce order unchanged", async ({
  page,
}) => {
  test.setTimeout(90000);
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned source amendment fixture",
  );
  const f = await customerAmendmentFixture(page, true);
  let before = await f.snapshot();
  await f.post(`loads/${f.loadId}/commands`, {
    action: "tender",
    carrier_id: f.originalCarrier,
    expected_version: before.loads.find((v) => v.id === f.loadId).version,
  });
  before = await f.snapshot();
  await f.post(`loads/${f.loadId}/commands`, {
    action: "withdraw_tender",
    expected_version: before.loads.find((v) => v.id === f.loadId).version,
  });
  await expect
    .poll(
      async () =>
        (await f.snapshot()).outbox.find(
          (v) =>
            v.load_id === f.loadId && v.destination === "stateset-sync-server",
        )?.status,
      { timeout: 15000 },
    )
    .toBe("local_only");
  const prePickup = await page.request.get(
    `${fixtureOrigin}/__sync_fixture/tracking`,
    { headers: f.fixtureHeaders },
  );
  const prePickupState = await prePickup.json();
  expect(prePickupState.order.status).toBe("pending");
  expect(prePickupState.tracking).toHaveLength(0);
  const changed = await page.request.patch(
    `${fixtureOrigin}/__sync_fixture/order`,
    { headers: f.fixtureHeaders, data: { status: "cancelled" } },
  );
  expect(changed.ok()).toBe(true);
  await f.post(`loads/${f.loadId}/source/refresh`, {});
  const a = await proposeAmendmentInBrowser(page, f);
  await acceptAmendmentInBrowser(page, a, true);
  await a.amended
    .getByRole("button", { name: "Activate customer amendment" })
    .click();
  await a.dialog.getByRole("button", { name: "Save quote action" }).click();
  await expect(a.dialog).not.toBeVisible();
  const w = await f.snapshot(),
    l = w.loads.find((v) => v.id === f.loadId),
    q = w.quotes.find((q) => q.id === a.quoteId);
  expect(l).toMatchObject({
    source_order: f.order,
    destination: {
      city: "Tacoma",
      address_lines: ["99 Amended Dock", "Gate B"],
    },
    sell_cents: 170001,
    quote_binding: { id: q.id },
    source_control: {
      held: false,
      commitment_ready: true,
      writeback_safe: false,
    },
  });
  expect(q.revisions[0].source_consent).toMatchObject({
    snapshot: { status: "cancelled" },
    authorize_transport: true,
  });
  const proof = w.source_reviews.filter((v) => v.load_id === l.id).at(-1);
  expect(proof).toMatchObject({
    status: "resolved",
    decision: {
      quote_id: q.id,
      decision: "authorize_transport",
      contact: "Verified customer transport lead",
      plan_snapshot: { destination: { city: "Tacoma" } },
    },
  });
  const upstream = await page.request.get(
    `${fixtureOrigin}/__sync_fixture/tracking`,
    { headers: f.fixtureHeaders },
  );
  const captured = await upstream.json();
  expect(captured.order.status).toBe("cancelled");
  expect(captured.tracking).toHaveLength(0);
  await a.amended.getByRole("button", { name: "Open booked shipment" }).click();
  const detail = page.getByRole("dialog"),
    source = detail.getByRole("region", { name: "Commerce source control" });
  await expect(
    detail.getByRole("button", { name: "Tender", exact: true }),
  ).toBeEnabled();
  await source.getByText(/Source decision history/).click();
  await expect(source).toContainText(`Amendment ${a.reference}`);
  if (process.env.FREIGHT_E2E_SCREENSHOTS === "true")
    await source
      .getByRole("status")
      .screenshot({ path: "../docs/screenshots/amendment-source.png" });
  await page.keyboard.press("Escape");
  await page.request.post("/api/session", {
    headers: { Origin: f.origin },
    data: { token: "freight-test-source-viewer-credential-at-least-32-chars" },
  });
  await page.reload();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Quotes", exact: true })
    .click();
  await page
    .getByRole("button", { name: `Open quote ${a.reference}`, exact: true })
    .click();
  await expect(
    a.amended.getByRole("button", {
      name: "Propose customer amendment",
      exact: true,
    }),
  ).toHaveCount(0);
  await f.post(`quotes/${q.id}/amendments`, {}, 403);
});

test("finance records paid-customer credit and actual partial refunds without reversing the receipt", async ({
  page,
}) => {
  test.setTimeout(90000);
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned customer refund workspace",
  );
  const origin = new URL(page.url()).origin,
    stamp = Date.now(),
    number = `REFUND-INV-${stamp}`,
    creditNumber = `REFUND-CR-${stamp}`;
  const post = async (path, data, expected = 200) => {
    const r = await page.request.post("/api/freight/" + path, {
      headers: { Origin: origin, "Idempotency-Key": randomUUID() },
      data,
    });
    expect(r.status(), await r.text()).toBe(expected);
    return (await r.json()).data;
  };
  const snapshot = async () => {
    const r = await page.request.get("/api/freight/workspace");
    expect(r.ok()).toBe(true);
    return r.json();
  };
  const day = new Date().toISOString().slice(0, 10);
  let load = await post("loads", {
    reference: `REFUND-LOAD-${stamp}`,
    customer_id: "c1",
    mode: "FTL",
    equipment: "Dry van",
    commodity: "Actual customer refund freight",
    weight_lbs: 1000,
    miles: 173,
    buy_cents: 118000,
    sell_cents: 149000,
    origin: {
      facility: "Portland dock",
      city: "Portland",
      state: "OR",
      date: day,
      address_lines: ["15 Dock Street"],
      postal_code: "97201",
      country: "US",
    },
    destination: {
      facility: "Seattle dock",
      city: "Seattle",
      state: "WA",
      date: day,
      address_lines: ["25 Receiving Ave"],
      postal_code: "98101",
      country: "US",
    },
  });
  load = await post(`loads/${load.id}/commands`, {
    action: "tender",
    expected_version: load.version,
    carrier_id: "ca4",
  });
  load = await post(`loads/${load.id}/commands`, {
    action: "accept",
    expected_version: load.version,
  });
  let w = await snapshot();
  const tender = w.tenders.find((t) => t.id === load.tender_id);
  await post("events", {
    event_id: randomUUID(),
    event_type: "edi.shipment_status.received",
    occurred_at: new Date().toISOString(),
    payload: {
      shipment_id: tender.shipment_reference,
      partner_id: "brex",
      carrier_scac: "BREX",
      latest_status: "D1",
      events: [{ status_code: "AF" }, { status_code: "D1" }],
    },
  });
  w = await snapshot();
  load = w.loads.find((l) => l.id === load.id);
  load = await post(`loads/${load.id}/commands`, {
    action: "attach_pod",
    expected_version: load.version,
    pod_reference:
      "Original signed receiver evidence for customer refund scenario",
  });
  await post(`loads/${load.id}/pod/review`, {
    expected_version: load.version,
    decision: "accepted",
    note: "Finance verified original external receiver evidence",
  });
  let bill = await post("bills", { load_id: load.id, number, due_date: day });
  bill = await post(`bills/${bill.id}/issue`, {
    expected_version: bill.version,
  });
  const receipt = await post("cash", {
    direction: "receipt",
    account: "operating-usd",
    external_reference: `GENUINE-RECEIPT-${stamp}`,
    amount_cents: 149000,
    currency: "USD",
    cleared_on: day,
    note: "Genuine original cleared receipt retained unchanged",
    allocations: [
      {
        target_id: bill.id,
        expected_version: bill.version,
        amount_cents: 149000,
      },
    ],
  });
  const uploaded = await page.request.post(
    `/api/freight/loads/${load.id}/documents?kind=other&filename=customer-credit-refund.pdf`,
    {
      headers: {
        Origin: origin,
        "Idempotency-Key": randomUUID(),
        "Content-Type": "application/pdf",
      },
      data: Buffer.from(
        "%PDF-1.7\nOriginal customer credit and refund confirmation\n%%EOF",
      ),
    },
  );
  expect(uploaded.ok()).toBe(true);
  const document = (await uploaded.json()).data;
  await page.request.post("/api/session", {
    headers: { Origin: origin },
    data: { token: "freight-test-finance-credential-at-least-32-chars" },
  });
  await page.reload();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Freight audit", exact: true })
    .click();
  await page
    .getByRole("tab", { name: "Customer billing", exact: true })
    .click();
  const row = page
      .getByRole("table")
      .filter({
        has: page.getByRole("columnheader", {
          name: "Received / credits",
          exact: true,
        }),
      })
      .getByRole("row")
      .filter({ has: page.getByText(number, { exact: true }) }),
    dialog = page.getByRole("dialog");
  await row.getByRole("button", { name: "Credit note", exact: true }).click();
  await dialog.getByLabel("Credit note number").fill(creditNumber);
  await dialog.getByLabel("Credit amount (USD)").fill("90.01");
  await dialog
    .getByLabel("Finance reason")
    .fill("Actual customer agreed service credit after original payment");
  const proof = async (ref) => {
    await dialog
      .getByLabel("Actual customer contact")
      .fill("Verified customer AP lead");
    await dialog.getByLabel("Customer confirmation reference").fill(ref);
    await dialog
      .getByLabel("Customer agreement or bank evidence reference")
      .fill("PRIVATE-CUSTOMER-ORIGINAL-EVIDENCE");
    await dialog
      .getByLabel("Customer evidence file (optional)")
      .selectOption(document.id);
  };
  await proof("PRIVATE-ACTUAL-CREDIT-CONFIRMATION");
  await dialog
    .getByRole("button", { name: "Post credit note", exact: true })
    .click();
  await expect(dialog).not.toBeVisible();
  await expect(row).toContainText("$90.01 customer refund due");
  const createdCreditRow = page
    .getByRole("row")
    .filter({ has: page.getByText(creditNumber, { exact: true }) });
  await createdCreditRow
    .getByText("Customer confirmation", { exact: true })
    .click();
  await expect(createdCreditRow).toContainText(
    "Actual customer agreed service credit after original payment",
  );
  await expect(createdCreditRow).toContainText(`Issued ${day}`);
  const evidenceResponse = await page.request.get(
    await createdCreditRow
      .getByRole("link", { name: "Download customer evidence" })
      .getAttribute("href"),
  );
  expect(evidenceResponse.ok()).toBe(true);
  expect(await evidenceResponse.body()).toEqual(
    Buffer.from(
      "%PDF-1.7\nOriginal customer credit and refund confirmation\n%%EOF",
    ),
  );

  w = await snapshot();
  expect(w.cash_entries.find((v) => v.id === receipt.id)).toMatchObject(
    receipt,
  );
  expect(w.cash_entries.find((v) => v.id === receipt.id).reversal).toBeNull();
  const refund = async (amount, reference) => {
    await row
      .getByRole("button", { name: "Record customer refund", exact: true })
      .click();
    await expect(dialog.getByLabel("Transaction direction")).toHaveValue(
      "customer_refund",
    );
    await dialog.getByLabel("Cleared bank reference").fill(reference);
    await dialog.getByLabel("Bank amount (USD)").fill(amount);
    await dialog.getByLabel(`Allocation amount ${number}`).fill(amount);
    await dialog
      .getByLabel("Reconciliation note")
      .fill("Actual bank statement confirms this customer refund cleared");
    await proof("PRIVATE-ACTUAL-REFUND-CONFIRMATION");
    await dialog
      .getByRole("button", { name: "Record reconciliation", exact: true })
      .click();
    await expect(dialog).not.toBeVisible();
  };
  await refund("30.01", `REFUND-ONE-${stamp}`);
  await expect(row).toContainText("$60.00 customer refund due");
  if (process.env.FREIGHT_E2E_SCREENSHOTS === "true")
    await row.screenshot({ path: "../docs/screenshots/customer-refunds.png" });
  await page.setViewportSize({ width: 390, height: 844 });
  await refund("60.00", `REFUND-TWO-${stamp}`);
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBe(true);
  w = await snapshot();
  let current = w.bills.find((v) => v.id === bill.id);
  expect(current).toMatchObject({
    status: "paid",
    snapshot: bill.snapshot,
    totals: {
      received_cents: 149000,
      credited_cents: 9001,
      refunded_cents: 9001,
      balance_cents: 0,
      refund_due_cents: 0,
    },
  });
  expect(w.cash_entries.find((v) => v.id === receipt.id)).toMatchObject(
    receipt,
  );
  const statement = await page.request.get(
    `/api/freight/bills/${bill.id}/statement`,
  );
  expect(statement.ok()).toBe(true);
  const html = await statement.text();
  expect(html).toContain("Reconciled customer refunds");
  expect(html).toContain("90.01");
  expect(html).not.toContain("PRIVATE-CUSTOMER-ORIGINAL-EVIDENCE");
  expect(html).not.toContain("PRIVATE-ACTUAL-CREDIT-CONFIRMATION");
  await page.setViewportSize({ width: 1280, height: 900 });
  const creditRow = page
    .getByRole("row")
    .filter({ has: page.getByText(creditNumber, { exact: true }) });
  await creditRow
    .getByRole("button", { name: "Reverse credit", exact: true })
    .click();
  await dialog
    .getByLabel("Finance reason")
    .fill(
      "Customer confirms actual service credit was issued to the wrong invoice; refunds remain actual bank history",
    );
  await dialog
    .getByRole("button", { name: "Confirm reversal", exact: true })
    .click();
  await expect(dialog).not.toBeVisible();
  w = await snapshot();
  current = w.bills.find((v) => v.id === bill.id);
  expect(current.totals.balance_cents).toBe(9001);
  expect(current.totals.refunded_cents).toBe(9001);
  expect(
    w.cash_entries
      .filter(
        (v) =>
          v.direction === "customer_refund" &&
          v.allocations.some((a) => a.target_id === bill.id),
      )
      .every((v) => v.reversal === null),
  ).toBe(true);
  await page.request.post("/api/session", {
    headers: { Origin: origin },
    data: { token: "freight-test-viewer-credential-at-least-32-chars" },
  });
  await page.reload();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Freight audit", exact: true })
    .click();
  await page
    .getByRole("tab", { name: "Customer billing", exact: true })
    .click();
  await expect(
    row.getByRole("button", { name: "Credit note", exact: true }),
  ).toBeDisabled();
  await post("cash", { direction: "customer_refund" }, 403);
});

test("carrier rate confirmations retain exact tender documents and handoff evidence controls unsent withdrawal", async ({
  page,
}) => {
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned workspace",
  );
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  const origin = new URL(page.url()).origin,
    reference = `RATE-CONFIRM-${Date.now()}`;
  const snapshot = async () => {
    const r = await page.request.get("/api/freight/workspace");
    expect(r.ok()).toBe(true);
    return r.json();
  };
  const post = async (path, data, status = 200) => {
    const r = await page.request.post(`/api/freight/${path}`, {
      headers: { Origin: origin, "Idempotency-Key": randomUUID() },
      data,
    });
    expect(r.status(), await r.text()).toBe(status);
    return (await r.json()).data;
  };
  const day = new Date(Date.now() + 86400000).toISOString().slice(0, 10);
  let l = await post("loads", {
    reference,
    customer_id: "c1",
    mode: "FTL",
    equipment: "Dry van",
    commodity: "Carrier document fixture",
    weight_lbs: 1000,
    miles: 173,
    buy_cents: 125001,
    sell_cents: 150001,
    origin: {
      facility: "Portland DC",
      city: "Portland",
      state: "OR",
      date: day,
      address_lines: ["15 Dock Street", "Suite 2"],
      postal_code: "97201",
      country: "US",
      contact: "Actual pickup contact",
      instructions: "Use gate B <script>escaped instructions</script>",
    },
    destination: {
      facility: "Seattle DC",
      city: "Seattle",
      state: "WA",
      date: day,
      address_lines: ["25 Receiving Ave"],
      postal_code: "98101",
      country: "US",
    },
  });
  l = await post(`loads/${l.id}/commands`, {
    action: "tender",
    expected_version: l.version,
    carrier_id: "ca4",
  });
  const proof = Buffer.from(
    "%PDF-1.7\nActual synthetic sent-message attachment evidence\n%%EOF",
  );
  const up = await page.request.post(
    `/api/freight/loads/${l.id}/documents?kind=other&filename=sent-rate-message.pdf`,
    {
      headers: {
        Origin: origin,
        "Idempotency-Key": randomUUID(),
        "Content-Type": "application/pdf",
      },
      data: proof,
    },
  );
  expect(up.ok()).toBe(true);
  const document = (await up.json()).data;
  await page.request.post("/api/session", {
    headers: { Origin: origin },
    data: { token: "freight-test-operations-credential-at-least-32-chars" },
  });
  await page.reload();
  const openLoad = async () => {
    await page
      .getByRole("navigation")
      .getByRole("button", { name: /Load board/ })
      .click();
    await page.getByLabel("Search loads").fill(reference);
    await page.getByRole("button", { name: reference, exact: true }).click();
  };
  await openLoad();
  const section = page.getByRole("region", {
      name: "Carrier rate confirmations",
      exact: true,
    }),
    dialog = page.getByRole("dialog", {
      name: /^(Issue carrier rate confirmation|Record actual carrier handoff|Resolve carrier handoff coordination|Correct erroneous handoff evidence)$/,
    });
  await section
    .getByRole("button", { name: "Issue carrier confirmation", exact: true })
    .click();
  await dialog
    .getByLabel("Actual carrier terms")
    .fill(
      "Actual carrier agreed inclusive USD terms; call dispatch before changes <script>never run</script>",
    );
  await dialog
    .getByLabel("Issuance or revision reason")
    .fill("Initial carrier confirmation against exact tender agreement");
  await dialog
    .getByRole("button", { name: "Issue rate confirmation", exact: true })
    .click();
  await expect(dialog).not.toBeVisible();
  let w = await snapshot(),
    first = w.rate_confirmations.find((v) => v.load_id === l.id),
    shipment = first.snapshot.shipment_reference;
  const card = section.getByRole("article", {
    name: `Carrier confirmation revision 1 ${shipment}`,
    exact: true,
  });
  await expect(card).toContainText("$1,250.01");
  expect(first.html).toBeUndefined();
  const original = await page.request.get(
    await card
      .getByRole("link", { name: "Download carrier confirmation", exact: true })
      .getAttribute("href"),
  );
  expect(original.ok()).toBe(true);
  const bytes = await original.body(),
    html = bytes.toString();
  expect(createHash("sha256").update(bytes).digest("hex")).toBe(first.sha256);
  for (const text of ["1250.01", "Suite 2", shipment, "&lt;script&gt;"])
    expect(html).toContain(text);
  expect(html).not.toContain("1500.01");
  expect(html).not.toContain("payment_instructions");
  await expect(
    page.getByRole("button", { name: "Withdraw unsent tender", exact: true }),
  ).toBeEnabled();
  const record = async (target, ref) => {
    await target
      .getByRole("button", { name: "Record actual handoff", exact: true })
      .click();
    await dialog
      .getByLabel("Actual carrier contact")
      .fill("Verified carrier dispatch contact");
    await dialog.getByLabel("External handoff reference").fill(ref);
    await dialog
      .getByLabel("Handoff evidence file (optional)")
      .selectOption(document.id);
    await dialog
      .getByLabel("Actual findings")
      .fill(
        "Inspected actual outbound message with this exact rate attachment",
      );
    await dialog
      .getByRole("button", { name: "Record handoff evidence", exact: true })
      .click();
    await expect(dialog).not.toBeVisible();
  };
  await record(card, "ACTUAL-CURRENT-RATE-HANDOFF");
  w = await snapshot();
  expect(w.loads.find((v) => v.id === l.id).status).toBe("tendered");
  expect(w.loads.find((v) => v.id === l.id).tender_control.can_withdraw).toBe(
    false,
  );
  expect(
    w.outbox
      .filter((v) => v.load_id === l.id && v.destination === "stateset-edi")
      .every((v) => v.total_attempts === 0),
  ).toBe(true);
  await card.getByText("Issued snapshot and history", { exact: true }).click();
  const evidence = await page.request.get(
    await card
      .getByRole("link", {
        name: "Download original handoff evidence",
        exact: true,
      })
      .getAttribute("href"),
  );
  expect(await evidence.body()).toEqual(proof);
  await card
    .getByRole("button", { name: "Correct erroneous handoff", exact: true })
    .click();
  await dialog
    .getByLabel("Actual findings")
    .fill(
      "Actual message log identifies wrong attachment; this recorded handoff was erroneous",
    );
  await dialog
    .getByRole("button", { name: "Retract erroneous evidence", exact: true })
    .click();
  await expect(dialog).not.toBeVisible();
  await expect(
    page.getByRole("button", { name: "Withdraw unsent tender", exact: true }),
  ).toBeEnabled();
  await section
    .getByRole("button", { name: "Issue revised confirmation", exact: true })
    .click();
  await dialog
    .getByLabel("Actual carrier terms")
    .fill(
      "Actual updated instructions: use gate C and call the receiving dock",
    );
  await dialog
    .getByLabel("Issuance or revision reason")
    .fill("Actual carrier requested different operating instructions");
  await dialog
    .getByRole("button", { name: "Issue rate confirmation", exact: true })
    .click();
  await expect(dialog).not.toBeVisible();
  await expect(card).toContainText("Historical");
  const unchanged = await page.request.get(
    await card
      .getByRole("link", { name: "Download carrier confirmation", exact: true })
      .getAttribute("href"),
  );
  expect(await unchanged.body()).toEqual(bytes);
  await record(card, "ACTUAL-OLD-DOCUMENT-HANDOFF-REPORTED-LATER");
  await expect(card.getByRole("status")).toContainText(
    "Resolve carrier coordination",
  );
  w = await snapshot();
  expect(w.loads.find((v) => v.id === l.id).tender_control.open_incidents).toBe(
    1,
  );
  await card
    .getByRole("button", { name: "Resolve handoff coordination", exact: true })
    .click();
  await dialog
    .getByLabel("Actual carrier contact")
    .fill("Actual carrier transport lead");
  await dialog
    .getByLabel("Carrier coordination reference")
    .fill("CARRIER-CONFIRMED-CURRENT-REVISION");
  await dialog
    .getByLabel("Actual findings")
    .fill(
      "Carrier confirms old attachment discarded; current revision received through actual operating channel",
    );
  await dialog
    .getByRole("button", { name: "Record carrier coordination", exact: true })
    .click();
  await expect(dialog).not.toBeVisible();
  w = await snapshot();
  expect(w.loads.find((v) => v.id === l.id).tender_control.open_incidents).toBe(
    0,
  );
  await page.setViewportSize({ width: 390, height: 844 });
  await section
    .getByRole("button", { name: "Issue revised confirmation", exact: true })
    .click();
  await expect(dialog.getByLabel("Actual carrier terms")).toBeVisible();
  expect(
    await dialog.evaluate((el) => el.scrollWidth <= el.clientWidth + 1),
  ).toBe(true);
  await page.keyboard.press("Escape");
  await expect(dialog).not.toBeVisible();
  await expect(section).toBeVisible();
  await page.setViewportSize({ width: 1280, height: 900 });
  if (process.env.FREIGHT_E2E_SCREENSHOTS === "true") {
    const currentCard = section.getByRole("article", {
      name: `Carrier confirmation revision 2 ${shipment}`,
      exact: true,
    });
    await currentCard.scrollIntoViewIfNeeded();
    await currentCard.screenshot({
      path: "../docs/screenshots/carrier-rate-confirmations.png",
    });
  }
  await page.request.post("/api/session", {
    headers: { Origin: origin },
    data: { token: "freight-test-viewer-credential-at-least-32-chars" },
  });
  await page.reload();
  await openLoad();
  await expect(
    section.getByRole("button", {
      name: "Issue revised confirmation",
      exact: true,
    }),
  ).toBeDisabled();
  await post(`loads/${l.id}/rate-confirmations`, {}, 403);
  await post(`rate-confirmations/${first.id}/handoffs`, {}, 403);
  expect(errors).toEqual([]);
});

test("partner portal scopes carrier offers and customer invoices and records native acceptance against the published revision", async ({
  page,
  browser,
}) => {
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned partner credentials",
  );
  test.setTimeout(90000);
  const origin = new URL(page.url()).origin,
    stamp = Date.now(),
    reference = `PARTNER-${stamp}`;
  const post = async (path, data) => {
    const r = await page.request.post(`/api/freight/${path}`, {
      headers: { Origin: origin, "Idempotency-Key": randomUUID() },
      data,
    });
    expect(r.ok(), await r.text()).toBe(true);
    return (await r.json()).data;
  };
  const snapshot = async () =>
    (await page.request.get("/api/freight/workspace")).json();
  const day = new Date(Date.now() + 86400000).toISOString().slice(0, 10);
  let l = await post("loads", {
    reference,
    customer_id: "c1",
    mode: "FTL",
    equipment: "Dry van",
    commodity: "Partner visibility fixture",
    weight_lbs: 1000,
    miles: 173,
    buy_cents: 125001,
    sell_cents: 150001,
    origin: {
      facility: "Portland DC",
      city: "Portland",
      state: "OR",
      date: day,
      address_lines: ["15 Dock Street"],
      postal_code: "97201",
      country: "US",
    },
    destination: {
      facility: "Seattle DC",
      city: "Seattle",
      state: "WA",
      date: day,
      address_lines: ["25 Receiving Ave"],
      postal_code: "98101",
      country: "US",
    },
  });
  l = await post(`loads/${l.id}/commands`, {
    action: "tender",
    expected_version: l.version,
    carrier_id: "ca4",
  });
  const rate = await post(`loads/${l.id}/rate-confirmations`, {
    expected_load_version: l.version,
    terms:
      "Actual partner carrier terms for this exact 1250.01 USD inclusive transport offer",
    reason: "PRIVATE-STAFF-PROCUREMENT-NOTE",
  });
  const partner = await browser.newContext({ baseURL: origin }),
    other = await browser.newContext({ baseURL: origin });
  try {
    const carrierPage = await partner.newPage();
    const errors = [];
    carrierPage.on("pageerror", (e) => errors.push(e.message));
    await carrierPage.goto("/portal");
    await carrierPage
      .getByLabel("Partner credential")
      .fill("freight-test-carrier-credential-at-least-32-chars");
    await carrierPage
      .getByRole("button", { name: "Open partner portal", exact: true })
      .click();
    await expect(
      carrierPage.getByRole("heading", {
        name: "Blue Ridge Express",
        exact: true,
      }),
    ).toBeVisible();
    let view = await (await partner.request.get("/api/freight/portal")).json();
    expect(view.offers.some((o) => o.load_id === l.id)).toBe(false);
    expect(
      (
        await partner.request.get(
          `/api/freight/portal/rate-confirmations/${rate.id}/statement`,
        )
      ).status(),
    ).toBe(404);
    await page.reload();
    await page
      .getByRole("navigation")
      .getByRole("button", { name: /Load board/ })
      .click();
    await page.getByLabel("Search loads").fill(reference);
    await page.getByRole("button", { name: reference, exact: true }).click();
    const rateSection = page.getByRole("region", {
      name: "Carrier rate confirmations",
      exact: true,
    });
    await rateSection
      .getByRole("button", { name: "Publish to carrier portal", exact: true })
      .click();
    const dialog = page.getByRole("dialog", {
      name: "Publish carrier portal offer",
      exact: true,
    });
    await expect(dialog).toContainText("prevents unsent withdrawal");
    await dialog
      .getByRole("button", { name: "Publish carrier offer", exact: true })
      .click();
    await expect(dialog).not.toBeVisible();
    let w = await snapshot();
    expect(w.loads.find((v) => v.id === l.id).tender_control.can_withdraw).toBe(
      false,
    );
    await carrierPage
      .getByRole("button", { name: "Refresh shipments", exact: true })
      .click();
    const card = carrierPage.getByRole("article", {
      name: `Carrier offer ${reference} revision 1`,
      exact: true,
    });
    await expect(card).toContainText("$1,250.01");
    view = await (await partner.request.get("/api/freight/portal")).json();
    expect(view.offers.find((v) => v.load_id === l.id).amount_cents).toBe(
      125001,
    );
    for (const secret of [
      "sell_cents",
      "150001",
      "PRIVATE-STAFF-PROCUREMENT",
      "billing_profile",
      "source_order",
      "outbox",
    ])
      expect(JSON.stringify(view)).not.toContain(secret);
    const download = await partner.request.get(
      await card
        .getByRole("link", {
          name: "Download carrier confirmation",
          exact: true,
        })
        .getAttribute("href"),
    );
    expect(download.ok()).toBe(true);
    expect(await download.text()).toBe(rate.html);
    for (const path of [
      "workspace",
      "finance/journal.csv",
      `rate-confirmations/${rate.id}/statement`,
    ])
      expect((await partner.request.get(`/api/freight/${path}`)).status()).toBe(
        403,
      );
    const otherSignIn = await other.request.post("/api/portal-session", {
      headers: { Origin: origin },
      data: {
        token: "freight-test-other-carrier-credential-at-least-32-chars",
      },
    });
    expect(otherSignIn.ok()).toBe(true);
    expect(
      (
        await other.request.get(
          `/api/freight/portal/rate-confirmations/${rate.id}/statement`,
        )
      ).status(),
    ).toBe(404);
    await carrierPage.setViewportSize({ width: 390, height: 844 });
    await card
      .getByRole("button", { name: "Accept carrier offer", exact: true })
      .click();
    const acceptance = carrierPage.getByRole("dialog", {
      name: "Accept published carrier offer",
      exact: true,
    });
    await acceptance.getByRole("checkbox").check();
    await acceptance
      .getByLabel("Your response reference")
      .fill(`NATIVE-CARRIER-${stamp}`);
    await acceptance
      .getByLabel("Carrier response note")
      .fill(
        "Carrier account lead reviewed and accepts this exact published carrier rate revision",
      );
    expect(
      await acceptance.evaluate((el) => el.scrollWidth <= el.clientWidth + 1),
    ).toBe(true);
    await acceptance
      .getByRole("button", { name: "Record carrier response", exact: true })
      .click();
    await expect(acceptance).not.toBeVisible();
    await expect(card).toContainText("accepted");
    w = await snapshot();
    expect(w.loads.find((v) => v.id === l.id).status).toBe("accepted");
    expect(w.portal_decisions.find((v) => v.load_id === l.id).recorded_by).toBe(
      "test:carrier",
    );
    expect(
      w.outbox
        .filter((v) => v.load_id === l.id && v.destination === "stateset-edi")
        .every((v) => v.total_attempts === 0),
    ).toBe(true);
    if (process.env.FREIGHT_E2E_SCREENSHOTS === "true") {
      await carrierPage.setViewportSize({ width: 1280, height: 900 });
      await card.screenshot({ path: "../docs/screenshots/carrier-portal.png" });
    }
    await carrierPage
      .getByRole("button", { name: "Sign out", exact: true })
      .click();
    await expect(carrierPage.getByLabel("Partner credential")).toBeVisible();
    expect((await partner.request.get("/api/freight/portal")).status()).toBe(
      401,
    );
    // An issued invoice belongs to the configured customer, including exact downloads.
    w = await snapshot();
    const delivered = w.loads.find((v) => v.id === "load_6");
    if (!delivered.pod_ready)
      await post(`loads/${delivered.id}/pod/review`, {
        expected_version: delivered.version,
        decision: "accepted",
        note: "Finance verified actual external receiver evidence for owned fixture",
      });
    // Earlier tests can have active billing on this shared synthetic shipment.
    let bill = w.bills.find(
      (b) => b.load_id === delivered.id && b.status !== "void" && b.issued_at,
    );
    if (!bill) {
      const draft = await post("bills", {
        load_id: delivered.id,
        number: `PARTNER-INVOICE-${stamp}`,
        due_date: new Date().toISOString().slice(0, 10),
      });
      bill = await post(`bills/${draft.id}/issue`, {
        expected_version: draft.version,
      });
    }
    await carrierPage
      .getByLabel("Partner credential")
      .fill("freight-test-customer-credential-at-least-32-chars");
    await carrierPage
      .getByRole("button", { name: "Open partner portal", exact: true })
      .click();
    await expect(
      carrierPage.getByRole("heading", {
        name: "Northstar Retail",
        exact: true,
      }),
    ).toBeVisible();
    const customerView = await (
      await partner.request.get("/api/freight/portal")
    ).json();
    expect(customerView.kind).toBe("customer");
    for (const secret of [
      "buy_cents",
      "margin",
      "source_order",
      "carrier_id",
      "portal_offers",
      "outbox",
    ])
      expect(JSON.stringify(customerView)).not.toContain(secret);
    expect(customerView.shipments.some((s) => s.id === l.id)).toBe(true);
    const invoice = carrierPage.getByRole("article", {
      name: `Customer invoice ${bill.number}`,
      exact: true,
    });
    const invoiceDownload = await partner.request.get(
      await invoice
        .getByRole("link", { name: "Download invoice", exact: true })
        .getAttribute("href"),
    );
    expect(invoiceDownload.ok()).toBe(true);
    expect(
      (
        await partner.request.post(
          `/api/freight/portal/offers/missing/response`,
          {
            headers: { Origin: origin, "Idempotency-Key": randomUUID() },
            data: {},
          },
        )
      ).status(),
    ).toBe(403);
    await other.request.post("/api/portal-session", {
      headers: { Origin: origin },
      data: {
        token: "freight-test-other-customer-credential-at-least-32-chars",
      },
    });
    expect(
      (
        await other.request.get(
          `/api/freight/portal/bills/${bill.id}/statement`,
        )
      ).status(),
    ).toBe(404);
    if (process.env.FREIGHT_E2E_SCREENSHOTS === "true")
      await invoice.screenshot({
        path: "../docs/screenshots/customer-portal.png",
      });
    expect(errors).toEqual([]);
  } finally {
    await partner.close();
    await other.close();
  }
});

test("partner Clerk sessions enforce counterparty organization membership and revocation through the gateway", async ({
  browser,
}) => {
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned identity fixture",
  );
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
      role: "org:carrier",
      member: true,
      org: "org_carrier",
      outage: false,
    });
    const { token } = await fixture("token", {
      counterparty_id: "ca1",
      role: "admin",
    });
    await context.addCookies([
      { name: "__session", value: token, url: origin, sameSite: "Strict" },
    ]);
    const path = "/api/freight/portal?organization_id=org_carrier";
    let r = await context.request.get(path);
    expect(r.ok(), await r.text()).toBe(true);
    const data = await r.json();
    expect(data.kind).toBe("carrier");
    expect(data.name).toBe("Blue Ridge Express");
    expect(data.access.subject).toBe("clerk:user_staff");
    expect(
      (
        await context.request.get(
          "/api/freight/workspace?organization_id=org_carrier",
        )
      ).status(),
    ).toBe(403);
    expect(
      (
        await context.request.get(
          "/api/freight/portal?organization_id=org_customer",
        )
      ).status(),
    ).toBe(403);
    expect(
      (
        await context.request.post("/api/portal-session", {
          headers: { Origin: origin },
          data: { token: "freight-test-carrier-credential-at-least-32-chars" },
        })
      ).status(),
    ).toBe(403);
    await fixture("control", { member: false });
    expect((await context.request.get(path)).status()).toBe(403);
    await fixture("control", { member: true, role: "org:operations" });
    expect((await context.request.get(path)).status()).toBe(403);
    await fixture("control", { role: "org:carrier", status: "revoked" });
    expect((await context.request.get(path)).status()).toBe(401);
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

test("carrier portal records physical progress and original POD on mobile while finance retains approval and file privacy", async ({
  page,
  browser,
}) => {
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned partner credentials",
  );
  test.setTimeout(90000);
  const origin = new URL(page.url()).origin,
    reference = `CARRIER-EVIDENCE-${Date.now()}`;
  const post = async (path, data) => {
    const r = await page.request.post(`/api/freight/${path}`, {
      headers: { Origin: origin, "Idempotency-Key": randomUUID() },
      data,
    });
    expect(r.ok(), await r.text()).toBe(true);
    return (await r.json()).data;
  };
  const day = new Date(Date.now() + 86400000).toISOString().slice(0, 10);
  let l = await post("loads", {
    reference,
    customer_id: "c1",
    mode: "FTL",
    equipment: "Dry van",
    commodity: "Native carrier evidence",
    weight_lbs: 1000,
    miles: 173,
    buy_cents: 125001,
    sell_cents: 150001,
    origin: {
      facility: "Portland DC",
      city: "Portland",
      state: "OR",
      date: day,
      address_lines: ["15 Dock Street"],
      postal_code: "97201",
      country: "US",
    },
    destination: {
      facility: "Seattle DC",
      city: "Seattle",
      state: "WA",
      date: day,
      address_lines: ["25 Receiving Ave"],
      postal_code: "98101",
      country: "US",
    },
  });
  l = await post(`loads/${l.id}/commands`, {
    action: "tender",
    expected_version: l.version,
    carrier_id: "ca4",
  });
  const rate = await post(`loads/${l.id}/rate-confirmations`, {
    expected_load_version: l.version,
    terms: "Actual published carrier transport terms",
    reason: "Private operations note",
  });
  l = (
    await (await page.request.get("/api/freight/workspace")).json()
  ).loads.find((v) => v.id === l.id);
  const offer = await post(`loads/${l.id}/portal-offers`, {
    expected_load_version: l.version,
    rate_confirmation_id: rate.id,
  });
  const partner = await browser.newContext({
      baseURL: origin,
      viewport: { width: 390, height: 844 },
    }),
    other = await browser.newContext({ baseURL: origin });
  const snapshot = async () =>
    (await page.request.get("/api/freight/workspace")).json();
  const pdf = Buffer.concat([
    Buffer.from("%PDF-1.7\nOwned synthetic carrier receiver proof\n"),
    Buffer.alloc(1024 * 1024, 32),
    Buffer.from("\n%%EOF"),
  ]);
  try {
    const p = await partner.newPage(),
      errors = [];
    p.on("pageerror", (e) => errors.push(e.message));
    await p.clock.install();
    await p.goto("/portal");
    await p
      .getByLabel("Partner credential")
      .fill("freight-test-carrier-credential-at-least-32-chars");
    await p.getByRole("button", { name: "Open partner portal" }).click();
    const card = p.getByRole("article", {
      name: `Carrier offer ${reference} revision 1`,
      exact: true,
    });
    await card
      .getByRole("button", { name: "Accept carrier offer", exact: true })
      .click();
    let dialog = p.getByRole("dialog");
    await dialog.getByRole("checkbox").check();
    await dialog
      .getByLabel("Your response reference")
      .fill("Actual carrier load commitment");
    await dialog
      .getByLabel("Carrier response note")
      .fill("Carrier dispatcher reviewed exact rate and assignment");
    await dialog
      .getByRole("button", { name: "Record carrier response", exact: true })
      .click();
    await expect(dialog).toHaveCount(0);
    const retainDuringRefresh = async (dialog, verify, database = false) => {
      const message = database
        ? "Database is temporarily busy. Retry the original request shortly."
        : "Workspace is busy. Please retry shortly.";
      const overload = async (route) =>
        route.fulfill({
          status: 503,
          contentType: "application/json",
          headers: { "Retry-After": "1" },
          body: JSON.stringify({
            error: {
              code: database
                ? "database_temporarily_unavailable"
                : "read_capacity_exceeded",
              message,
            },
          }),
        });
      await p.route("**/api/freight/portal", overload);
      try {
        await p.clock.fastForward(30000);
        await expect(
          p.getByRole("main", { includeHidden: true }).getByText(message, {
            exact: true,
          }),
        ).toBeVisible();
        await expect(dialog).toBeVisible();
        await expect(
          p.getByRole("article", {
            name: `Carrier offer ${reference} revision 1`,
            exact: true,
            includeHidden: true,
          }),
        ).toBeVisible();
        await expect(p.getByLabel("Partner credential")).toHaveCount(0);
        await verify();
      } finally {
        await p.unroute("**/api/freight/portal", overload);
      }
    };
    const observe = async (kind, minutes, location) => {
      await card
        .getByRole("button", { name: "Report shipment progress", exact: true })
        .click();
      const d = p.getByRole("dialog");
      await d.getByLabel("Shipment observation").selectOption(kind);
      await d
        .getByLabel("Observed time (UTC)")
        .fill(
          new Date(Date.now() - minutes * 60000).toISOString().slice(0, 16),
        );
      await d.getByLabel("Observed location").fill(location);
      await d
        .getByLabel("Evidence reference")
        .fill(`Actual carrier ${kind} reference`);
      await d
        .getByLabel("Carrier evidence note")
        .fill(`Actual driver and receiver ${kind} evidence`);
      if (kind === "pickup")
        await retainDuringRefresh(d, async () => {
          await expect(d.getByLabel("Observed location")).toHaveValue(location);
          await expect(d.getByLabel("Evidence reference")).toHaveValue(
            `Actual carrier ${kind} reference`,
          );
          await expect(d.getByLabel("Carrier evidence note")).toHaveValue(
            `Actual driver and receiver ${kind} evidence`,
          );
        });
      await d
        .getByRole("button", {
          name: "Record shipment observation",
          exact: true,
        })
        .click();
      await expect(d).toHaveCount(0);
    };
    await observe("pickup", 20, "Portland, OR");
    await expect(card.locator(".badge")).toHaveText("in_transit");
    await observe("location", 10, "Tacoma, WA");
    await expect(card).toContainText("Latest location: Tacoma, WA");
    await observe("delivery", 5, "Seattle, WA");
    await expect(card.locator(".badge")).toHaveText("delivered");
    let w = await snapshot();
    expect(w.loads.find((v) => v.id === l.id).pod_ready).toBe(false);
    const invoice = await post("invoices", {
      load_id: l.id,
      number: `NATIVE-${Date.now()}`,
      amount_cents: 125001,
      currency: "USD",
    });
    const approve = () =>
      page.request.post(`/api/freight/invoices/${invoice.id}/approve`, {
        headers: { Origin: origin, "Idempotency-Key": randomUUID() },
        data: {},
      });
    expect((await approve()).status()).toBe(422);
    await card.getByRole("button", { name: "Submit POD", exact: true }).click();
    dialog = p.getByRole("dialog");
    await dialog.getByLabel("POD file").setInputFiles({
      name: "original-carrier-receiver.pdf",
      mimeType: "application/pdf",
      buffer: pdf,
    });
    await dialog
      .getByLabel("Evidence reference")
      .fill("Actual signed receiver file from carrier");
    await dialog
      .getByLabel("Carrier evidence note")
      .fill("Carrier lead supplied original receiver delivery evidence");
    await retainDuringRefresh(
      dialog,
      async () => {
        expect(
          await dialog
            .getByLabel("POD file")
            .evaluate((input) => input.files[0].size),
        ).toBe(pdf.length);
        await expect(dialog.getByLabel("Evidence reference")).toHaveValue(
          "Actual signed receiver file from carrier",
        );
        await expect(dialog.getByLabel("Carrier evidence note")).toHaveValue(
          "Carrier lead supplied original receiver delivery evidence",
        );
      },
      true,
    );
    await dialog
      .getByRole("button", { name: "Submit POD for review", exact: true })
      .click();
    await expect(dialog).toHaveCount(0);
    await expect(card).toContainText("POD review: pending review");
    expect((await approve()).status()).toBe(422);
    await card.getByText("Your shipment evidence (4)", { exact: true }).click();
    const link = card.getByRole("link", {
      name: "Download your POD: original-carrier-receiver.pdf",
      exact: true,
    });
    await expect(link).toBeVisible();
    const url = await link.getAttribute("href"),
      download = await partner.request.get(url);
    expect(download.ok()).toBe(true);
    expect(await download.body()).toEqual(pdf);
    expect(download.headers()["x-content-sha256"]).toBe(
      createHash("sha256").update(pdf).digest("hex"),
    );
    expect(download.headers()["content-disposition"]).toContain("attachment;");
    expect(
      (
        await partner.request.get(`/api/freight/documents/missing/content`)
      ).status(),
    ).toBe(403);
    expect(
      (
        await partner.request.post(
          `/api/freight/portal/offers/${offer.id}/tracking`,
          {
            headers: {
              Origin: "https://wrong.example",
              "Idempotency-Key": randomUUID(),
            },
            data: {},
          },
        )
      ).status(),
    ).toBe(403);
    await other.request.post("/api/portal-session", {
      headers: { Origin: origin },
      data: {
        token: "freight-test-other-carrier-credential-at-least-32-chars",
      },
    });
    expect((await other.request.get(url)).status()).toBe(404);
    expect(
      (
        await other.request.post(
          `/api/freight/portal/offers/${offer.id}/tracking`,
          {
            headers: { Origin: origin, "Idempotency-Key": randomUUID() },
            data: {
              expected_load_version: 1,
              kind: "pickup",
              observed_at: new Date().toISOString(),
              location: "Wrong party",
              reference: "Crossed carrier",
              note: "Wrong account attempt",
            },
          },
        )
      ).status(),
    ).toBe(404);
    w = await snapshot();
    const doc = w.documents.find(
      (v) => v.id === w.loads.find((v) => v.id === l.id).pod_document_id,
    );
    expect(doc.uploaded_by).toBe("test:carrier");
    expect(doc.reviews).toEqual([]);
    await post(`documents/${doc.id}/review`, {
      expected_version: doc.version,
      decision: "accepted",
      note: "PRIVATE-FINANCE-ONLY receiver proof inspected",
    });
    expect((await approve()).ok()).toBe(true);
    const settlement = await post(`loads/${l.id}/settlement`, {});
    expect(settlement.amount_cents).toBe(125001);
    await p
      .getByRole("button", { name: "Refresh shipments", exact: true })
      .click();
    await expect(card).toContainText("POD review: accepted");
    await expect(
      card.getByRole("button", { name: "Submit POD", exact: true }),
    ).toHaveCount(0);
    const view = await (
      await partner.request.get("/api/freight/portal")
    ).json();
    expect(JSON.stringify(view)).not.toContain("PRIVATE-FINANCE-ONLY");
    expect(JSON.stringify(view)).not.toContain("sell_cents");
    expect(
      await p.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBe(true);
    if (process.env.FREIGHT_E2E_SCREENSHOTS === "true")
      await card.screenshot({
        path: "../docs/screenshots/carrier-pod-mobile.png",
      });
    let heldRefresh,
      refreshCount = 0;
    const olderStarted = new Promise((resolve) => {
      heldRefresh = resolve;
    });
    await p.route("**/api/freight/portal", async (route) => {
      if (++refreshCount === 1) {
        heldRefresh(route);
        return;
      }
      await route.fulfill({
        status: 401,
        contentType: "application/json",
        body: JSON.stringify({
          error: { message: "A current partner session is required." },
        }),
      });
    });
    await p
      .getByRole("button", { name: "Refresh shipments", exact: true })
      .click();
    const older = await olderStarted;
    await p
      .getByRole("button", { name: "Refresh shipments", exact: true })
      .click();
    await expect(p.getByLabel("Partner credential")).toBeVisible();
    const lateResponse = p.waitForResponse(
      (r) => r.url().endsWith("/api/freight/portal") && r.status() === 200,
    );
    await older.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(view),
    });
    await (await lateResponse).finished();
    await p.clock.runFor(100);
    await expect(p.getByLabel("Partner credential")).toBeVisible();
    await expect(card).toHaveCount(0);
    await expect(p.getByRole("dialog")).toHaveCount(0);
    expect(errors).toEqual([]);
  } finally {
    await partner.close();
    await other.close();
  }
});

test("customer portal accepts the exact published quote on mobile while booking and procurement stay in staff control", async ({
  page,
  browser,
}) => {
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned customer credentials",
  );
  test.setTimeout(90000);
  const origin = new URL(page.url()).origin,
    reference = `CUSTOMER-NATIVE-${Date.now()}`;
  const post = async (path, data) => {
    const r = await page.request.post(`/api/freight/${path}`, {
      headers: { Origin: origin, "Idempotency-Key": randomUUID() },
      data,
    });
    expect(r.ok(), await r.text()).toBe(true);
    return (await r.json()).data;
  };
  const day = new Date(Date.now() + 2 * 86400000).toISOString().slice(0, 10);
  let q = await post("quotes", {
    reference,
    plan: {
      customer_id: "c1",
      mode: "FTL",
      equipment: "Dry van",
      commodity: "Customer portal freight",
      weight_lbs: 1000,
      miles: 173,
      origin: {
        facility: "Portland DC",
        city: "Portland",
        state: "OR",
        date: day,
        address_lines: ["15 Dock Street"],
        postal_code: "97201",
        country: "US",
      },
      destination: {
        facility: "Seattle DC",
        city: "Seattle",
        state: "WA",
        date: day,
        address_lines: ["25 Receiving Ave"],
        postal_code: "98101",
        country: "US",
      },
    },
    sell_cents: 150001,
    valid_until: new Date(Date.now() + 12 * 3600000).toISOString(),
    terms: "Actual inclusive customer terms for the exact published shipment",
  });
  q = await post(`quotes/${q.id}/offers`, {
    expected_version: q.version,
    carrier_id: "ca4",
    amount_cents: 125001,
    valid_until: new Date(Date.now() + 24 * 3600000).toISOString(),
    contact: "PRIVATE-CARRIER-CONTACT",
    reference: "PRIVATE-CARRIER-REFERENCE",
    note: "PRIVATE-PROCUREMENT-DETAILS",
  });
  q = await post(`quotes/${q.id}/issue`, {
    expected_version: q.version,
    offer_id: q.revisions[0].offers[0].id,
  });
  await page.reload();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Quotes", exact: true })
    .click();
  await page
    .getByRole("button", { name: `Open quote ${reference}`, exact: true })
    .click();
  const detail = page.getByRole("region", {
    name: `Quote details ${reference}`,
    exact: true,
  });
  await detail
    .getByRole("button", { name: "Publish to customer portal", exact: true })
    .click();
  let dialog = page.getByRole("dialog");
  await dialog.getByRole("checkbox").check();
  // The staff action publishes an immutable document; no native decision is fabricated.
  await dialog
    .getByRole("button", { name: "Save quote action", exact: true })
    .click();
  await expect(dialog).toHaveCount(0);
  await expect(detail).toContainText("This revision is published");
  const partner = await browser.newContext({
      baseURL: origin,
      viewport: { width: 390, height: 844 },
    }),
    other = await browser.newContext({ baseURL: origin });
  try {
    const p = await partner.newPage(),
      errors = [];
    p.on("pageerror", (e) => errors.push(e.message));
    await p.goto("/portal");
    await p
      .getByLabel("Partner credential")
      .fill("freight-test-customer-credential-at-least-32-chars");
    await p.getByRole("button", { name: "Open partner portal" }).click();
    const card = p.getByRole("article", {
      name: `Customer quote ${reference} revision 1`,
      exact: true,
    });
    await expect(card).toContainText("$1,500.01");
    await expect(card).not.toContainText("PRIVATE-CARRIER");
    await expect(card).not.toContainText("$1,250.01");
    const docurl = await card
      .getByRole("link", { name: "Download published quote", exact: true })
      .getAttribute("href");
    const original = await partner.request.get(docurl);
    expect(original.ok()).toBe(true);
    const bytes = await original.body();
    expect(original.headers()["x-content-sha256"]).toBe(
      createHash("sha256").update(bytes).digest("hex"),
    );
    await card
      .getByRole("button", { name: "Accept customer quote", exact: true })
      .click();
    dialog = p.getByRole("dialog");
    await dialog.getByRole("checkbox").check();
    await dialog
      .getByLabel("Your customer contact")
      .fill("Actual customer transportation lead");
    await dialog
      .getByLabel("Your decision reference")
      .fill("Actual native customer approval");
    await dialog
      .getByLabel("Customer decision note")
      .fill("Customer reviewed the exact shipment plan and inclusive price");
    await dialog
      .getByRole("button", { name: "Record customer decision", exact: true })
      .click();
    await expect(dialog).toHaveCount(0);
    await expect(card.locator(".badge")).toHaveText("accepted");
    let w = await (await page.request.get("/api/freight/workspace")).json();
    let accepted = w.quotes.find((v) => v.id === q.id);
    expect(accepted.booked_load_id).toBeNull();
    expect(accepted.revisions[0].decision.recorded_by).toBe("test:customer");
    expect(
      w.customer_quote_decisions.filter((d) => d.quote_id === q.id),
    ).toHaveLength(1);
    const publicView = await (
      await partner.request.get("/api/freight/portal")
    ).json();
    expect(JSON.stringify(publicView)).not.toContain("PRIVATE-PROCUREMENT");
    expect(JSON.stringify(publicView)).not.toContain("buy_cents");
    expect((await partner.request.get("/api/freight/workspace")).status()).toBe(
      403,
    );
    await other.request.post("/api/portal-session", {
      headers: { Origin: origin },
      data: {
        token: "freight-test-other-customer-credential-at-least-32-chars",
      },
    });
    expect((await other.request.get(docurl)).status()).toBe(404);
    const booked = await post(`quotes/${q.id}/book`, {
      expected_version: accepted.version,
    });
    w = await (await page.request.get("/api/freight/workspace")).json();
    const l = w.loads.find((v) => v.id === booked.booked_load_id);
    expect([l.buy_cents, l.sell_cents]).toEqual([125001, 150001]);
    expect(l.carrier_id).toBeNull();
    await p
      .getByRole("button", { name: "Refresh shipments", exact: true })
      .click();
    await expect(card.locator(".badge")).toHaveText("booked");
    expect(await (await partner.request.get(docurl)).body()).toEqual(bytes);
    await expect(
      card.getByRole("button", {
        name: "Withdraw quote or acceptance",
        exact: true,
      }),
    ).toHaveCount(0);
    expect(
      await p.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBe(true);
    if (process.env.FREIGHT_E2E_SCREENSHOTS === "true")
      await card.screenshot({
        path: "../docs/screenshots/customer-quote-portal-mobile.png",
      });
    expect(errors).toEqual([]);
  } finally {
    await partner.close();
    await other.close();
  }
});

test("customer portal amendment acceptance binds fresh cancelled-source consent without changing the existing agreement early", async ({
  page,
  browser,
}, testInfo) => {
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned source/customer fixture",
  );
  test.setTimeout(90000);
  const f = await customerAmendmentFixture(page, true, true);
  await page.request.patch(`${fixtureOrigin}/__sync_fixture/order`, {
    headers: f.fixtureHeaders,
    data: { status: "cancelled" },
  });
  // A background source verification can race this setup verification. Keep
  // one original key/body and repeat only the exact definite CAS refusal.
  const refreshKey = randomUUID(),
    refreshBody = {},
    originalQuote = (await f.snapshot()).quotes.find(
      (quote) => quote.id === f.quote.id,
    ),
    setupVerifications = [];
  let sourceVerified = false;
  for (let attempt = 0; attempt < 5; attempt++) {
    const response = await page.request.post(
        `/api/freight/loads/${f.loadId}/source/refresh`,
        {
          headers: { Origin: f.origin, "Idempotency-Key": refreshKey },
          data: refreshBody,
        },
      ),
      payload = await response.json();
    setupVerifications.push({
      attempt: attempt + 1,
      status: response.status(),
    });
    if (response.status() === 200) {
      expect(payload.data.load_id).toBe(f.loadId);
      expect(payload.data.snapshot.status).toBe("cancelled");
      sourceVerified = true;
      break;
    }
    expect(response.status()).toBe(409);
    expect(payload).toEqual({
      error: { message: "Sync source check changed; retry refresh" },
    });
    const unchanged = await f.snapshot();
    expect(
      unchanged.loads.find((load) => load.id === f.loadId).sell_cents,
    ).toBe(150001);
    expect(unchanged.quotes.find((quote) => quote.id === f.quote.id)).toEqual(
      originalQuote,
    );
  }
  expect(sourceVerified).toBe(true);
  await testInfo.attach("customer-amendment-setup-verifications", {
    body: Buffer.from(JSON.stringify(setupVerifications)),
    contentType: "application/json",
  });
  const a = await proposeAmendmentInBrowser(page, f);
  await a.amended
    .getByRole("button", { name: "Publish to customer portal", exact: true })
    .click();
  await a.dialog.getByRole("checkbox").check();
  await a.dialog
    .getByRole("button", { name: "Save quote action", exact: true })
    .click();
  await expect(a.dialog).toHaveCount(0);
  const partner = await browser.newContext({
    baseURL: f.origin,
    viewport: { width: 390, height: 844 },
  });
  try {
    const p = await partner.newPage(),
      errors = [];
    p.on("pageerror", (e) => errors.push(e.message));
    await p.goto("/portal");
    await p
      .getByLabel("Partner credential")
      .fill("freight-test-customer-credential-at-least-32-chars");
    await p.getByRole("button", { name: "Open partner portal" }).click();
    const card = p.getByRole("article", {
      name: `Customer quote ${a.reference} revision 1`,
      exact: true,
    });
    await expect(card).toContainText("cancelled");
    await expect(card).toContainText("$1,700.01");
    await expect(card).toContainText("existing agreement stays effective");
    const fill = async () => {
      await card
        .getByRole("button", { name: "Accept customer quote", exact: true })
        .click();
      const d = p.getByRole("dialog");
      await d
        .getByRole("checkbox", { name: /reviewed the exact published/ })
        .check();
      await d
        .getByLabel("Your customer contact")
        .fill("Actual customer transportation lead");
      await d
        .getByLabel("Your decision reference")
        .fill("Native amended transportation authorization");
      await d
        .getByLabel("Customer decision note")
        .fill(
          "Customer reviewed the amended plan and current cancelled source status",
        );
      return d;
    };
    let d = await fill();
    const authorization = d.getByRole("checkbox", {
      name: /explicitly authorize/,
    });
    await expect(authorization).toBeVisible();
    await d
      .getByRole("button", { name: "Record customer decision", exact: true })
      .click();
    await expect(d).toBeVisible();
    await authorization.check();
    await page.request.patch(`${fixtureOrigin}/__sync_fixture/order`, {
      headers: f.fixtureHeaders,
      data: { lineItems: [{ sku: "ORIGINAL-GOODS", quantity: 3 }] },
    });
    await d
      .getByRole("button", { name: "Record customer decision", exact: true })
      .click();
    await expect(d.getByRole("alert")).toContainText("Sync source changed");
    let w = await f.snapshot();
    expect(w.loads.find((l) => l.id === f.loadId).sell_cents).toBe(150001);
    expect(w.quotes.find((q) => q.id === a.quoteId).status).toBe("issued");
    await d.getByRole("button", { name: "Cancel", exact: true }).click();
    const refreshAndReview = async () => {
      await card
        .getByRole("button", { name: "Verify commerce order", exact: true })
        .click();
      await expect(p.getByRole("status")).toContainText(
        "Commerce order status verified",
      );
      d = await fill();
      await d.getByRole("checkbox", { name: /explicitly authorize/ }).check();
    };
    await refreshAndReview();
    // Replay one definite refusal observed from the real API when its source
    // check races another refresh. This request is not forwarded or applied.
    let replayedSourceConflict = false;
    let decisionRequests = 0;
    await p.route(
      "**/api/freight/portal/customer-quotes/*/response",
      async (route) => {
        if (route.request().method() === "POST") decisionRequests++;
        if (!replayedSourceConflict && route.request().method() === "POST") {
          replayedSourceConflict = true;
          await route.fulfill({
            status: 409,
            contentType: "application/json",
            body: JSON.stringify({
              error: { message: "Sync source check changed; retry refresh" },
            }),
          });
          return;
        }
        await route.continue();
      },
    );
    const sourceAttempts = [];
    let accepted = false;
    for (let attempt = 0; attempt < 4; attempt++) {
      const responsePromise = p.waitForResponse(
        (response) =>
          response.request().method() === "POST" &&
          new URL(response.url()).pathname.endsWith("/response") &&
          new URL(response.url()).pathname.includes(
            "/api/freight/portal/customer-quotes/",
          ),
      );
      await d
        .getByRole("button", { name: "Record customer decision", exact: true })
        .click();
      const response = await responsePromise;
      sourceAttempts.push({ attempt: attempt + 1, status: response.status() });
      if (response.status() === 200) {
        expect(decisionRequests).toBe(sourceAttempts.length);
        accepted = true;
        break;
      }
      expect(response.status()).toBe(409);
      expect(await response.json()).toEqual({
        error: { message: "Sync source check changed; retry refresh" },
      });
      await expect(d.getByRole("alert")).toContainText(
        "Sync source check changed; retry refresh",
      );
      w = await f.snapshot();
      expect(w.loads.find((l) => l.id === f.loadId).sell_cents).toBe(150001);
      expect(w.quotes.find((q) => q.id === a.quoteId).status).toBe("issued");
      const unchangedPortal = await (
        await partner.request.get("/api/freight/portal")
      ).json();
      expect(
        unchangedPortal.quotes.find((q) => q.quote_id === a.quoteId).decisions,
      ).toEqual([]);
      expect(decisionRequests).toBe(sourceAttempts.length);
      if (attempt < 3) {
        await d.getByRole("button", { name: "Cancel", exact: true }).click();
        await refreshAndReview();
      }
    }
    expect(replayedSourceConflict).toBe(true);
    expect(accepted).toBe(true);
    await testInfo.attach("customer-amendment-source-recovery", {
      body: Buffer.from(
        JSON.stringify({
          replayedSourceConflict,
          decisionRequests,
          sourceAttempts,
        }),
      ),
      contentType: "application/json",
    });
    await expect(d).toHaveCount(0);
    await expect(card.locator(".badge")).toHaveText("accepted");
    w = await f.snapshot();
    expect(w.loads.find((l) => l.id === f.loadId).sell_cents).toBe(150001);
    expect(
      w.quotes.find((q) => q.id === a.quoteId).revisions[0].source_consent,
    ).toMatchObject({
      authorize_transport: true,
      snapshot: { status: "cancelled", lineItems: [{ quantity: 3 }] },
    });
    const view = await (
      await partner.request.get("/api/freight/portal")
    ).json();
    expect(JSON.stringify(view)).not.toContain(
      "ORIGINAL-AMENDMENT-PRIVATE-OFFER",
    );
    expect(JSON.stringify(view)).not.toContain('"metadata"');
    await card.getByText("Your quote decisions (1)", { exact: true }).click();
    await expect(card).toContainText("Source at acceptance:");
    await expect(card).toContainText(
      "Customer explicitly authorized the amended transportation plan",
    );
    if (process.env.FREIGHT_E2E_SCREENSHOTS === "true")
      await card.screenshot({
        path: "../docs/screenshots/customer-amendment-portal.png",
      });
    await page.reload();
    await page
      .getByRole("navigation")
      .getByRole("button", { name: "Quotes", exact: true })
      .click();
    await page
      .getByRole("button", { name: `Open quote ${a.reference}`, exact: true })
      .click();
    await a.amended
      .getByRole("button", { name: "Activate customer amendment", exact: true })
      .click();
    await a.dialog
      .getByRole("button", { name: "Save quote action", exact: true })
      .click();
    await expect(a.dialog).toHaveCount(0);
    w = await f.snapshot();
    const l = w.loads.find((l) => l.id === f.loadId);
    expect(l.sell_cents).toBe(170001);
    expect(l.destination.city).toBe("Tacoma");
    expect(l.source_order).toEqual(f.order);
    const source = await (
      await page.request.get(`${fixtureOrigin}/__sync_fixture/tracking`, {
        headers: f.fixtureHeaders,
      })
    ).json();
    expect(source.order.status).toBe("cancelled");
    expect(source.tracking).toHaveLength(0);
    expect(errors).toEqual([]);
  } finally {
    await partner.close();
  }
});

test("staff shares exact shipment files with scoped partners and revokes future access without undoing carrier communication", async ({
  page,
  browser,
}) => {
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned partner credentials",
  );
  test.setTimeout(90000);
  const origin = new URL(page.url()).origin,
    reference = `SHARED-FILES-${Date.now()}`,
    filename = "shared-receiver-évidence.pdf",
    bytes = Buffer.from(
      "%PDF-1.7\nOwned synthetic shipment bill of lading evidence\n%%EOF",
    );
  const post = async (path, data) => {
    const r = await page.request.post(`/api/freight/${path}`, {
      headers: { Origin: origin, "Idempotency-Key": randomUUID() },
      data,
    });
    expect(r.ok(), await r.text()).toBe(true);
    return (await r.json()).data;
  };
  const snapshot = async () =>
      (await page.request.get("/api/freight/workspace")).json(),
    day = new Date(Date.now() + 86400000).toISOString().slice(0, 10);
  let l = await post("loads", {
    reference,
    customer_id: "c1",
    mode: "FTL",
    equipment: "Dry van",
    commodity: "Shared shipment files",
    weight_lbs: 1000,
    miles: 173,
    buy_cents: 125001,
    sell_cents: 150001,
    origin: {
      facility: "Portland DC",
      city: "Portland",
      state: "OR",
      date: day,
      address_lines: ["15 Dock Street"],
      postal_code: "97201",
      country: "US",
    },
    destination: {
      facility: "Seattle DC",
      city: "Seattle",
      state: "WA",
      date: day,
      address_lines: ["25 Receiving Ave"],
      postal_code: "98101",
      country: "US",
    },
  });
  l = await post(`loads/${l.id}/commands`, {
    action: "tender",
    carrier_id: "ca4",
    expected_version: l.version,
  });
  const open = async () => {
    await page.reload();
    await page
      .getByRole("navigation")
      .getByRole("button", { name: /^Load board/ })
      .click();
    await page.getByLabel("Search loads").fill(reference);
    await page.getByRole("button", { name: reference, exact: true }).click();
  };
  await page.clock.install();
  await open();
  const parent = page.getByRole("dialog");
  await parent
    .getByLabel("Document type", { exact: true })
    .selectOption("bill_of_lading");
  await parent.getByLabel("Shipment document file").setInputFiles({
    name: filename,
    mimeType: "application/pdf",
    buffer: bytes,
  });
  await parent
    .getByRole("button", { name: "Upload document", exact: true })
    .click();
  const card = parent.getByRole("article").filter({
    has: page.getByRole("link", {
      name: `Download ${filename}`,
      exact: true,
    }),
  });
  await expect(card).toBeVisible();
  const publish = async (recipient, message) => {
    await card
      .getByRole("button", { name: "Share file with partner", exact: true })
      .click();
    const d = page.getByRole("dialog", {
      name: "Share original file with partner",
      exact: true,
    });
    await d.getByLabel("File recipient").selectOption(recipient);
    await d.getByLabel("Message for recipient").fill(message);
    await d
      .getByLabel("Private content review note")
      .fill(
        "PRIVATE-SHARE-REVIEW inspected actual bytes, filename and selected recipient",
      );
    await d.getByRole("checkbox").check();
    await d
      .getByRole("button", { name: "Publish file access", exact: true })
      .click();
    await expect(d).toHaveCount(0);
    await expect(parent).toBeVisible();
  };
  const revoke = async (name) => {
    await card.getByText(/Partner file access/).click();
    const history = card
      .locator(".file-share-history")
      .filter({ hasText: name })
      .filter({
        has: page.getByRole("button", {
          name: "Revoke file access",
          exact: true,
        }),
      });
    await history
      .getByRole("button", { name: "Revoke file access", exact: true })
      .click();
    const d = page.getByRole("dialog", {
      name: "Revoke partner file access",
      exact: true,
    });
    await d
      .getByLabel("Private revocation reason")
      .fill("PRIVATE-REVOCATION actual staff correction of disclosure");
    await d
      .getByRole("button", { name: "Revoke partner access", exact: true })
      .click();
    await expect(d).toHaveCount(0);
  };
  await card
    .getByRole("button", { name: "Share file with partner", exact: true })
    .click();
  const staleDialog = page.getByRole("dialog", {
    name: "Share original file with partner",
    exact: true,
  });
  await staleDialog.getByLabel("File recipient").selectOption("customer");
  await staleDialog
    .getByLabel("Message for recipient")
    .fill("Actual reviewed original customer disclosure");
  await staleDialog
    .getByLabel("Private content review note")
    .fill("Actual file and recipient inspection before concurrent update");
  await staleDialog.getByRole("checkbox").check();
  await post(`loads/${l.id}/commands`, {
    action: "exception",
    expected_version: l.version,
    note: "PRIVATE-STALE-SHARE-CONTEXT-CHANGED",
  });
  await page.clock.fastForward(31000);
  await expect(parent.locator(".exception-box")).toContainText(
    "PRIVATE-STALE-SHARE-CONTEXT-CHANGED",
  );
  await staleDialog
    .getByRole("button", { name: "Publish file access", exact: true })
    .click();
  await expect(staleDialog.getByRole("alert")).toContainText("Load changed");
  expect(
    (await snapshot()).document_shares.filter((s) => s.load_id === l.id),
  ).toHaveLength(0);
  await staleDialog
    .getByRole("button", { name: "Cancel", exact: true })
    .click();
  await publish("customer", "Original shipment file for your customer account");
  let w = await snapshot();
  let share = w.document_shares.find(
    (s) => s.load_id === l.id && s.audience === "customer",
  );
  expect(w.loads.find((v) => v.id === l.id).tender_control.can_withdraw).toBe(
    true,
  );
  const partner = await browser.newContext({
      baseURL: origin,
      viewport: { width: 390, height: 844 },
    }),
    other = await browser.newContext({ baseURL: origin });
  try {
    const p = await partner.newPage(),
      errors = [];
    p.on("pageerror", (e) => errors.push(e.message));
    await p.goto("/portal");
    await p
      .getByLabel("Partner credential")
      .fill("freight-test-customer-credential-at-least-32-chars");
    await p.getByRole("button", { name: "Open partner portal" }).click();
    const shared = p.getByRole("article", {
      name: `Shared file ${filename} for ${reference}`,
      exact: true,
    });
    await expect(shared).toContainText("Available");
    const url = await shared
      .getByRole("link", { name: "Download shared file", exact: true })
      .getAttribute("href");
    const download = await partner.request.get(url);
    expect(download.ok()).toBe(true);
    expect(await download.body()).toEqual(bytes);
    expect(download.headers()["x-content-sha256"]).toBe(
      createHash("sha256").update(bytes).digest("hex"),
    );
    expect(download.headers()["content-disposition"]).toContain("attachment;");
    expect(download.headers()["x-content-type-options"]).toBe("nosniff");
    const view = await (
      await partner.request.get("/api/freight/portal")
    ).json();
    expect(JSON.stringify(view)).not.toContain("PRIVATE-SHARE-REVIEW");
    expect(JSON.stringify(view)).not.toContain("review_note");
    expect(JSON.stringify(view)).not.toContain("download_authorizations");
    await other.request.post("/api/portal-session", {
      headers: { Origin: origin },
      data: {
        token: "freight-test-other-customer-credential-at-least-32-chars",
      },
    });
    expect((await other.request.get(url)).status()).toBe(404);
    w = await snapshot();
    expect(
      w.document_shares.find((s) => s.id === share.id).download_authorizations,
    ).toMatchObject([{ authorized_by: "test:customer" }]);
    expect(
      await p.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBe(true);
    if (process.env.FREIGHT_E2E_SCREENSHOTS === "true")
      await shared.screenshot({
        path: "../docs/screenshots/shared-file-portal-mobile.png",
      });
    await revoke("Northstar Retail");
    expect((await partner.request.get(url)).status()).toBe(404);
    await p
      .getByRole("button", { name: "Refresh shipments", exact: true })
      .click();
    await expect(shared).toContainText("Access revoked");
    await expect(
      shared.getByRole("link", { name: "Download shared file", exact: true }),
    ).toHaveCount(0);
    w = await snapshot();
    expect(
      w.document_shares.find((s) => s.id === share.id).download_authorizations,
    ).toHaveLength(1);
    const tender = w.loads.find((v) => v.id === l.id).tender_id;
    await publish(
      `carrier:${tender}`,
      "Original shipment instructions for this carrier tender",
    );
    w = await snapshot();
    expect(w.loads.find((v) => v.id === l.id).tender_control.can_withdraw).toBe(
      false,
    );
    expect(w.loads.find((v) => v.id === l.id).status).toBe("tendered");
    await other.request.post("/api/portal-session", {
      headers: { Origin: origin },
      data: { token: "freight-test-carrier-credential-at-least-32-chars" },
    });
    const carrierView = await (
      await other.request.get("/api/freight/portal")
    ).json();
    const carrierShare = carrierView.files.find(
      (f) => f.load_reference === reference,
    );
    expect(carrierShare.available).toBe(true);
    const cd = await other.request.get(
      `/api/freight/portal/files/${carrierShare.id}/content`,
    );
    expect(await cd.body()).toEqual(bytes);
    // Publication remains communication after the URL is revoked, including at zero EDI attempts.
    const details = card.locator(".file-sharing > details");
    if ((await details.getAttribute("open")) !== null)
      await card.getByText(/Partner file access/).click();
    await revoke("Blue Ridge Express");
    w = await snapshot();
    expect(w.loads.find((v) => v.id === l.id).tender_control.can_withdraw).toBe(
      false,
    );
    expect(
      w.outbox
        .filter((j) => j.load_id === l.id)
        .every((j) => j.total_attempts === 0),
    ).toBe(true);
    expect(
      await (
        await page.request.get(
          `/api/freight/documents/${share.document_id}/content`,
        )
      ).body(),
    ).toEqual(bytes);
    await page.request.post("/api/session", {
      headers: { Origin: origin },
      data: { token: "freight-test-viewer-credential-at-least-32-chars" },
    });
    await open();
    await expect(
      page
        .getByRole("dialog")
        .getByRole("button", { name: "Share file with partner", exact: true }),
    ).toBeDisabled();
    expect(errors).toEqual([]);
  } finally {
    await partner.close();
    await other.close();
  }
});

test("carrier portal authenticates exact release and retention decisions while pickup and replacement ownership remain protected", async ({
  page,
  browser,
}) => {
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned carrier credentials",
  );
  test.setTimeout(90000);
  const origin = new URL(page.url()).origin,
    reference = `NATIVE-RELEASE-${Date.now()}`;
  const post = async (path, data) => {
    const r = await page.request.post(`/api/freight/${path}`, {
      headers: { Origin: origin, "Idempotency-Key": randomUUID() },
      data,
    });
    expect(r.ok(), await r.text()).toBe(true);
    return (await r.json()).data;
  };
  const snapshot = async () =>
    (await page.request.get("/api/freight/workspace")).json();
  const create = async (ref) => {
    const day = new Date(Date.now() + 86400000).toISOString().slice(0, 10);
    let l = await post("loads", {
      reference: ref,
      customer_id: "c1",
      mode: "FTL",
      equipment: "Dry van",
      commodity: "Carrier release proof",
      weight_lbs: 1000,
      miles: 173,
      buy_cents: 125001,
      sell_cents: 150001,
      origin: {
        facility: "Portland DC",
        city: "Portland",
        state: "OR",
        date: day,
        address_lines: ["15 Dock Street"],
        postal_code: "97201",
        country: "US",
      },
      destination: {
        facility: "Seattle DC",
        city: "Seattle",
        state: "WA",
        date: day,
        address_lines: ["25 Receiving Ave"],
        postal_code: "98101",
        country: "US",
      },
    });
    l = await post(`loads/${l.id}/commands`, {
      action: "tender",
      carrier_id: "ca4",
      expected_version: l.version,
    });
    return post(`loads/${l.id}/commands`, {
      action: "accept",
      expected_version: l.version,
    });
  };
  const open = async (ref) => {
    await page.reload();
    await page
      .getByRole("navigation")
      .getByRole("button", { name: /^Load board/ })
      .click();
    await page.getByLabel("Search loads").fill(ref);
    await page.getByRole("button", { name: ref, exact: true }).click();
  };
  const l = await create(reference);
  const parent = page.getByRole("dialog");
  const request = async (ref) => {
    await open(ref);
    await parent
      .getByRole("button", { name: "Request carrier release", exact: true })
      .click();
    await parent
      .getByLabel("Release reason")
      .fill("PRIVATE-NATIVE-RELEASE-REASON current carrier coordination");
    await parent
      .getByRole("button", { name: "Create release request", exact: true })
      .click();
    await parent
      .getByRole("button", {
        name: "Publish release request to carrier portal",
        exact: true,
      })
      .click();
    const d = page.getByRole("dialog", {
      name: "Publish carrier release request",
      exact: true,
    });
    await d
      .getByLabel("Message for carrier")
      .fill(
        "Please confirm your no-pickup/no-custody release of this exact carrier assignment",
      );
    await d.getByRole("checkbox").check();
    await d
      .getByRole("button", {
        name: "Publish carrier release request",
        exact: true,
      })
      .click();
    await expect(d).toHaveCount(0);
    return (await snapshot()).release_publications
      .filter((p) => p.snapshot.load_reference === ref)
      .at(-1);
  };
  let publication = await request(reference);
  const partner = await browser.newContext({
      baseURL: origin,
      viewport: { width: 390, height: 844 },
    }),
    other = await browser.newContext({ baseURL: origin });
  try {
    const p = await partner.newPage(),
      errors = [];
    p.on("pageerror", (e) => errors.push(e.message));
    await p.goto("/portal");
    await p
      .getByLabel("Partner credential")
      .fill("freight-test-carrier-credential-at-least-32-chars");
    await p.getByRole("button", { name: "Open partner portal" }).click();
    const card = () =>
      p.getByRole("article", {
        name: `Carrier release request ${reference} ${publication.id}`,
        exact: true,
      });
    let c = card();
    await expect(c).toContainText("Return this shipment to planning");
    const url = await c
      .getByRole("link", {
        name: "Download exact release request",
        exact: true,
      })
      .getAttribute("href");
    const original = await partner.request.get(url);
    expect(original.ok()).toBe(true);
    const bytes = await original.body();
    expect(original.headers()["x-content-sha256"]).toBe(
      createHash("sha256").update(bytes).digest("hex"),
    );
    await other.request.post("/api/portal-session", {
      headers: { Origin: origin },
      data: {
        token: "freight-test-other-carrier-credential-at-least-32-chars",
      },
    });
    expect((await other.request.get(url)).status()).toBe(404);
    expect(
      (
        await partner.request.post(
          `/api/freight/portal/releases/${publication.id}/response`,
          {
            headers: {
              Origin: "https://wrong.example",
              "Idempotency-Key": randomUUID(),
            },
            data: {},
          },
        )
      ).status(),
    ).toBe(403);
    const respond = async (decision) => {
      const c = card();
      await c
        .getByRole("button", {
          name:
            decision === "release"
              ? "Confirm carrier release"
              : "Retain assignment",
          exact: true,
        })
        .click();
      const d = p.getByRole("dialog");
      if (decision === "release") await d.getByRole("checkbox").check();
      else {
        await expect(d).toContainText("Your carrier will remain assigned.");
        await expect(d).not.toContainText("Release will");
      }
      await d
        .getByLabel("Carrier contact", { exact: true })
        .fill("Actual authenticated carrier lead");
      await d
        .getByLabel("Carrier response reference", { exact: true })
        .fill(`Native carrier ${decision} reference`);
      await d
        .getByLabel("Carrier release findings", { exact: true })
        .fill(
          "Carrier checked its driver and the exact published assignment before responding",
        );
      if (
        decision === "release" &&
        process.env.FREIGHT_E2E_SCREENSHOTS === "true"
      )
        await d.screenshot({
          path: "../docs/screenshots/carrier-release-portal-mobile.png",
        });
      await d
        .getByRole("button", {
          name: "Record carrier release response",
          exact: true,
        })
        .click();
      await expect(d).toHaveCount(0);
    };
    await respond("retain");
    await expect(c.locator(".badge")).toHaveText("retained");
    let w = await snapshot();
    expect(w.loads.find((v) => v.id === l.id).status).toBe("accepted");
    expect(w.loads.find((v) => v.id === l.id).carrier_id).toBe("ca4");
    publication = await request(reference);
    await p
      .getByRole("button", { name: "Refresh shipments", exact: true })
      .click();
    c = card();
    await respond("release");
    await expect(c.locator(".badge")).toHaveText("confirmed");
    w = await snapshot();
    const released = w.loads.find((v) => v.id === l.id);
    expect(released.status).toBe("planned");
    expect(released.carrier_id).toBeNull();
    const proof = w.release_responses.filter((r) => r.load_id === l.id).at(-1);
    expect(proof.recorded_by).toBe("test:carrier");
    expect(proof.no_custody).toBe(true);
    expect(proof.application.mode).toBe("native");
    const currentDoc = await partner.request.get(
      `/api/freight/portal/releases/${publication.id}/statement`,
    );
    expect(
      createHash("sha256")
        .update(await currentDoc.body())
        .digest("hex"),
    ).toBe(publication.sha256);
    expect(await (await partner.request.get(url)).body()).toEqual(bytes);
    const replacement = await post(`loads/${l.id}/commands`, {
      action: "tender",
      carrier_id: "ca1",
      expected_version: released.version,
    });
    w = await snapshot();
    const replacementTender = w.tenders.find(
      (t) => t.id === replacement.tender_id,
    );
    await p
      .getByRole("button", { name: "Refresh shipments", exact: true })
      .click();
    const view = await (
      await partner.request.get("/api/freight/portal")
    ).json();
    expect(JSON.stringify(view)).not.toContain("PRIVATE-NATIVE-RELEASE-REASON");
    expect(JSON.stringify(view)).not.toContain(
      replacementTender.shipment_reference,
    );
    expect(
      view.release_requests
        .filter((v) => v.snapshot.load_reference === reference)
        .every(
          (v) => v.load_version === null && !v.can_release && !v.can_retain,
        ),
    ).toBe(true);
    const moving = await create(reference + "-PICKUP");
    publication = await request(reference + "-PICKUP");
    w = await snapshot();
    const tender = w.tenders.find((t) => t.id === moving.tender_id);
    await post("events", {
      event_id: randomUUID(),
      event_type: "edi.shipment_status.received",
      occurred_at: new Date().toISOString(),
      payload: {
        shipment_id: tender.shipment_reference,
        partner_id: tender.carrier.partner_id,
        carrier_scac: tender.carrier.scac,
        latest_status: "AF",
      },
    });
    await p
      .getByRole("button", { name: "Refresh shipments", exact: true })
      .click();
    const blocked = p.getByRole("article", {
      name: `Carrier release request ${reference}-PICKUP ${publication.id}`,
      exact: true,
    });
    await expect(blocked).toContainText("blocked by movement");
    await expect(
      blocked.getByRole("button", {
        name: "Confirm carrier release",
        exact: true,
      }),
    ).toHaveCount(0);
    expect(
      await p.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBe(true);
    expect(errors).toEqual([]);
  } finally {
    await partner.close();
    await other.close();
  }
});
