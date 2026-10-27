const { test, expect } = require("@playwright/test");
const { randomUUID, createHash } = require("node:crypto");
test("carrier contract rates retain exact revisions and source fresh capacity without changing existing procurement", async ({
  page,
  browser,
}) => {
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned credential services",
  );
  test.setTimeout(90000);
  await page.goto("/");
  await page
    .getByLabel("Workspace credential")
    .fill("freight-test-operator-credential-at-least-32-chars");
  await page.getByRole("button", { name: "Open workspace" }).click();
  await expect(
    page.getByRole("heading", { name: "Overview", exact: true }),
  ).toBeVisible();
  const origin = new URL(page.url()).origin,
    number = `CONTRACT-${Date.now()}`,
    day = new Date(Date.now() + 2 * 86400000).toISOString().slice(0, 10),
    errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
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
  const draft = (reference) => ({
    reference,
    plan: {
      customer_id: "c1",
      mode: "FTL",
      equipment: "Dry van",
      commodity: "Contract freight",
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
    terms: "Actual inclusive customer agreement",
  });
  const q = await post("quotes", draft(number + "-QUOTE"));
  await page.reload();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Quotes", exact: true })
    .click();
  await page
    .getByRole("button", { name: "Record carrier agreement", exact: true })
    .click();
  const d = page.getByRole("dialog", {
    name: "Record carrier agreement",
    exact: true,
  });
  await d.getByLabel("Agreement number", { exact: true }).fill(number);
  await d.getByLabel("Agreement carrier", { exact: true }).selectOption("ca4");
  await d.getByLabel("Customer scope", { exact: true }).selectOption("c1");
  await d.getByLabel("Origin state/province", { exact: true }).fill("OR");
  await d.getByLabel("Origin postal code", { exact: true }).fill("97201");
  await d.getByLabel("Destination state/province", { exact: true }).fill("WA");
  await d.getByLabel("Destination postal code", { exact: true }).fill("98101");
  await d.getByLabel("Maximum distance (miles)", { exact: true }).fill("200");
  await d
    .getByLabel("Effective through", { exact: true })
    .fill(new Date(Date.now() + 30 * 86400000).toISOString().slice(0, 10));
  await d.getByLabel("Inclusive rate", { exact: true }).fill("1250.01");
  await d
    .getByLabel("Original inclusive rate terms", { exact: true })
    .fill("Inclusive fuel and linehaul. PRIVATE-CARRIER-CONTRACT-SOURCE-TERMS");
  await d
    .getByLabel("Agreement revision reason", { exact: true })
    .fill("Actual reviewed external agreement");
  const proof = async (dialog, reference) => {
    await dialog
      .getByLabel("Actual carrier agreement contact", { exact: true })
      .fill("Actual carrier contracting lead");
    await dialog
      .getByLabel("Original carrier agreement reference", { exact: true })
      .fill(reference);
    await dialog
      .getByLabel("Private agreement findings", { exact: true })
      .fill("PRIVATE-ORIGINAL-AGREEMENT inspected original carrier terms");
    await dialog.getByRole("checkbox").check();
  };
  await proof(d, "ORIGINAL-LANE-AGREEMENT");
  await d
    .getByRole("button", { name: "Save carrier agreement", exact: true })
    .click();
  await expect(d).toHaveCount(0);
  let w = await snapshot(),
    c = w.carrier_contracts.find((c) => c.number === number);
  const originalUrl = `/api/freight/carrier-contracts/${c.id}/statement?revision=1`,
    original = await page.request.get(originalUrl),
    bytes = await original.body();
  expect(original.headers()["x-content-sha256"]).toBe(
    createHash("sha256").update(bytes).digest("hex"),
  );
  await page
    .getByRole("button", { name: `Open quote ${number}-QUOTE`, exact: true })
    .click();
  const parent = page.getByRole("region", {
    name: `Quote details ${number}-QUOTE`,
    exact: true,
  });
  await expect(parent).toBeVisible();
  const sourcing = parent.getByRole("region", {
    name: "Contract rate sourcing",
    exact: true,
  });
  await sourcing
    .getByRole("button", { name: "Find contract rates", exact: true })
    .click();
  let card = sourcing.getByRole("article", {
    name: `Contract rate ${number}`,
    exact: true,
  });
  await expect(card).toContainText("$1,250.01");
  await card
    .getByRole("button", { name: "Confirm capacity at this rate", exact: true })
    .click();
  const capacity = page.getByRole("dialog", {
    name: "Confirm carrier capacity at agreed rate",
    exact: true,
  });
  const fillCapacity = async (ref) => {
    await capacity
      .getByLabel("Capacity confirmation contact", { exact: true })
      .fill("Actual carrier capacity lead");
    await capacity
      .getByLabel("Capacity confirmation reference", { exact: true })
      .fill(ref);
    await capacity
      .getByLabel("Capacity confirmation findings", { exact: true })
      .fill(
        "Carrier confirmed actual equipment capacity and this exact inclusive rate",
      );
    await capacity.getByRole("checkbox").check();
  };
  await fillCapacity("CAPACITY-BEFORE-CONTRACT-REVISION");
  // A staff review stays bound to the exact terms even if another instance revises the agreement.
  const next = {
    number,
    carrier_id: "ca4",
    expected_version: c.version,
    rate: { ...c.revisions[0].rate, amount_cents: 130001 },
    contact: "Actual contracting lead",
    reference: "REVISED-CARRIER-AGREEMENT",
    note: "New terms confirmed independently",
    reason: "New commercial period pricing",
  };
  c = await post(`carrier-contracts/${c.id}/revise`, next);
  await capacity
    .getByRole("button", {
      name: "Record confirmed contract rate",
      exact: true,
    })
    .click();
  await expect(capacity.getByRole("alert")).toContainText("changed;");
  expect(
    (await snapshot()).quotes.find((v) => v.id === q.id).revisions[0].offers,
  ).toHaveLength(0);
  await capacity.getByRole("button", { name: "Cancel", exact: true }).click();
  await sourcing
    .getByRole("button", { name: "Refresh contract rates", exact: true })
    .click();
  card = sourcing.getByRole("article", {
    name: `Contract rate ${number}`,
    exact: true,
  });
  await expect(card).toContainText("$1,300.01");
  await card
    .getByRole("button", { name: "Confirm capacity at this rate", exact: true })
    .click();
  await fillCapacity("ACTUAL-CURRENT-CAPACITY");
  await capacity
    .getByRole("button", {
      name: "Record confirmed contract rate",
      exact: true,
    })
    .click();
  await expect(capacity).toHaveCount(0);
  await expect(parent).toContainText("Source: carrier agreement revision 2");
  w = await snapshot();
  let quote = w.quotes.find((v) => v.id === q.id);
  expect(quote.revisions[0].offers[0]).toMatchObject({
    amount_cents: 130001,
    rate_source: { agreement_id: c.id, revision: 2 },
    evidence: { reference: "ACTUAL-CURRENT-CAPACITY" },
  });
  expect(quote.booked_load_id).toBeNull();
  // Withdraw the catalogue entry through the actual UI; the separately confirmed offer remains.
  await page.reload();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Quotes", exact: true })
    .click();
  const agreementCard = page.getByRole("article", {
    name: `Carrier agreement ${number}`,
    exact: true,
  });
  await agreementCard
    .getByRole("button", { name: "Revise carrier agreement", exact: true })
    .click();
  const revisionDialog = page.getByRole("dialog", {
    name: "Revise carrier agreement",
    exact: true,
  });
  await expect(
    revisionDialog.getByLabel("Agreement number", { exact: true }),
  ).toBeDisabled();
  await revisionDialog
    .getByLabel("Inclusive rate", { exact: true })
    .fill("1400.01");
  await revisionDialog
    .getByLabel("Agreement revision reason", { exact: true })
    .fill("Further terms reviewed without replacing the confirmed offer");
  await proof(revisionDialog, "THIRD-REVIEWED-CARRIER-AGREEMENT");
  await revisionDialog
    .getByRole("button", { name: "Save carrier agreement", exact: true })
    .click();
  await expect(revisionDialog).toHaveCount(0);
  await expect(agreementCard).toContainText("$1,400.01");
  await agreementCard
    .getByRole("button", { name: "Withdraw for new sourcing", exact: true })
    .click();
  const withdrawal = page.getByRole("dialog", {
    name: "Withdraw carrier agreement",
    exact: true,
  });
  await proof(withdrawal, "STOP-NEW-SOURCING");
  await withdrawal
    .getByRole("button", { name: "Withdraw agreement", exact: true })
    .click();
  await expect(withdrawal).toHaveCount(0);
  await expect(agreementCard).toContainText("withdrawn");
  quote = await post(`quotes/${q.id}/issue`, {
    expected_version: quote.version,
    offer_id: quote.revisions[0].offers[0].id,
  });
  quote = await post(`quotes/${q.id}/decision`, {
    expected_version: quote.version,
    decision: "accept",
    contact: "Customer transport lead",
    reference: "Actual customer agreement",
    note: "Accepted current inclusive sell price",
  });
  quote = await post(`quotes/${q.id}/book`, {
    expected_version: quote.version,
  });
  w = await snapshot();
  expect(w.loads.find((v) => v.id === quote.booked_load_id)).toMatchObject({
    buy_cents: 130001,
    sell_cents: 150001,
    status: "planned",
  });
  expect(await (await page.request.get(originalUrl)).body()).toEqual(bytes);
  const historyDraft = draft(number + "-HISTORY");
  const history = await post("quotes", historyDraft);
  const historyUrl = `/api/freight/quotes/${history.id}/statement?revision=1`;
  const historyBytes = await (await page.request.get(historyUrl)).body();
  const { reference: historyReference, ...revisionFields } = historyDraft;
  await post(`quotes/${history.id}/revise`, {
    ...revisionFields,
    expected_version: history.version,
    sell_cents: 160001,
    reason: "Retain original customer quote revision through gateway",
  });
  expect(await (await page.request.get(historyUrl)).body()).toEqual(
    historyBytes,
  );
  expect(
    await (
      await page.request.get(`/api/freight/quotes/${history.id}/statement`)
    ).text(),
  ).toContain("USD 1600.01");
  const viewer = await browser.newContext({
      baseURL: origin,
      viewport: { width: 390, height: 844 },
    }),
    partner = await browser.newContext({ baseURL: origin });
  try {
    await viewer.request.post("/api/session", {
      headers: { Origin: origin },
      data: { token: "freight-test-viewer-credential-at-least-32-chars" },
    });
    const v = await viewer.newPage();
    await v.goto("/");
    await v
      .getByRole("button", { name: "Toggle navigation", exact: true })
      .click();
    await v
      .getByRole("navigation")
      .getByRole("button", { name: "Quotes", exact: true })
      .click();
    await expect(
      v.getByRole("button", { name: "Record carrier agreement", exact: true }),
    ).toBeDisabled();
    const mobile = v.getByRole("article", {
      name: `Carrier agreement ${number}`,
      exact: true,
    });
    await expect(mobile).toBeVisible();
    expect(
      await v.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBe(true);
    if (process.env.FREIGHT_E2E_SCREENSHOTS === "true")
      await mobile.screenshot({
        path: "../docs/screenshots/carrier-contract-mobile.png",
      });
    await partner.request.post("/api/portal-session", {
      headers: { Origin: origin },
      data: { token: "freight-test-customer-credential-at-least-32-chars" },
    });
    expect((await partner.request.get(originalUrl)).status()).toBe(403);
    const portal = await (
      await partner.request.get("/api/freight/portal")
    ).json();
    expect(JSON.stringify(portal)).not.toContain("PRIVATE-CARRIER-CONTRACT");
  } finally {
    await viewer.close();
    await partner.close();
  }
  expect(errors).toEqual([]);
});
