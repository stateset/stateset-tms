const { test, expect } = require("@playwright/test");
const { randomUUID, createHash } = require("node:crypto");

test("reviewed native movement times retain original reports, reject stale reviews, and remain private across mobile portals", async ({
  page,
  browser,
}) => {
  test.skip(
    process.env.FREIGHT_E2E_EXTERNAL === "true",
    "requires owned credential services",
  );
  test.setTimeout(90000);
  page.setDefaultTimeout(12000);
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
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
  const origin = new URL(page.url()).origin,
    reference = `MOVEMENT-${Date.now()}`;
  const snapshot = async () =>
    (await page.request.get("/api/freight/workspace")).json();
  const past = (minutes) =>
    new Date(Date.now() - minutes * 60000).toISOString().slice(0, 16);
  const post = async (path, data) => {
    const r = await page.request.post(`/api/freight/${path}`, {
      headers: { Origin: origin, "Idempotency-Key": randomUUID() },
      data,
    });
    expect(r.ok(), await r.text()).toBe(true);
    return (await r.json()).data;
  };
  const day = new Date(Date.now() + 86400000).toISOString().slice(0, 10);
  const stop = (facility, city, state, postal_code) => ({
    facility,
    city,
    state,
    postal_code,
    country: "US",
    date: day,
    address_lines: ["10 Synthetic Evidence Street"],
  });
  const created = await post("loads", {
    reference,
    customer_id: "c1",
    mode: "FTL",
    equipment: "Dry van",
    commodity: "Movement evidence freight",
    weight_lbs: 1000,
    miles: 173,
    buy_cents: 125001,
    sell_cents: 150001,
    origin: stop("Portland origin", "Portland", "OR", "97201"),
    destination: stop("Seattle receiver", "Seattle", "WA", "98101"),
  });
  let w = await snapshot(),
    load = w.loads.find((l) => l.id === created.id);
  const current = async () =>
    (await snapshot()).loads.find((l) => l.id === created.id);
  await post(`loads/${load.id}/commands`, {
    action: "tender",
    expected_version: load.version,
    carrier_id: "ca4",
  });
  const rate = await post(`loads/${load.id}/rate-confirmations`, {
    expected_load_version: (await current()).version,
    terms: "Actual original two-stop carrier agreement",
    reason: "Reviewed original rate",
  });
  const offer = await post(`loads/${load.id}/portal-offers`, {
    expected_load_version: (await current()).version,
    rate_confirmation_id: rate.id,
  });
  const rateUrl = `/api/freight/rate-confirmations/${rate.id}/statement`,
    rateBytes = await (await page.request.get(rateUrl)).body();
  expect(createHash("sha256").update(rateBytes).digest("hex")).toBe(
    rate.sha256,
  );
  const carrier = await browser.newContext({
      baseURL: origin,
      viewport: { width: 390, height: 844 },
    }),
    customer = await browser.newContext({
      baseURL: origin,
      viewport: { width: 390, height: 844 },
    }),
    viewer = await browser.newContext({
      baseURL: origin,
      viewport: { width: 390, height: 844 },
    });
  try {
    const partnerPost = async (path, data) => {
      const r = await carrier.request.post(`/api/freight/${path}`, {
        headers: { Origin: origin, "Idempotency-Key": randomUUID() },
        data,
      });
      expect(r.ok(), await r.text()).toBe(true);
      return (await r.json()).data;
    };
    for (const [context, token] of [
      [carrier, "carrier"],
      [customer, "customer"],
      [viewer, "viewer"],
    ]) {
      context.setDefaultTimeout(12000);
      const r = await context.request.post(
        token === "viewer" ? "/api/session" : "/api/portal-session",
        {
          headers: { Origin: origin },
          data: { token: `freight-test-${token}-credential-at-least-32-chars` },
        },
      );
      expect(r.ok()).toBe(true);
    }
    await partnerPost(`portal/offers/${offer.id}/response`, {
      expected_load_version: (await current()).version,
      decision: "accept",
      rate_confirmation_id: offer.rate_confirmation_id,
      rate_sha256: offer.rate_sha256,
      reference: "Actual native acceptance",
      note: "Original carrier reviewed exact plan and rate",
    });
    const originals = [];
    for (const [kind, minutes] of [
      ["pickup", 20],
      ["delivery", 5],
    ])
      originals.push(
        await partnerPost(`portal/offers/${offer.id}/tracking`, {
          expected_load_version: (await current()).version,
          kind,
          observed_at: past(minutes) + ":00Z",
          location: `Actual ${kind} location`,
          reference: `ORIGINAL-CARRIER-${kind}`,
          note: "Original authenticated carrier movement evidence",
        }),
      );
    load = await current();
    const physical = (l) =>
      Object.fromEntries(
        [
          "status",
          "location",
          "last_tracking_at",
          "tender_id",
          "carrier_id",
          "buy_cents",
          "sell_cents",
          "sync_order_id",
          "order_number",
        ].map((k) => [k, l[k]]),
      );
    const originalPhysical = physical(load);
    const openLoad = async (p) => {
      await p.goto("/");
      if (p.viewportSize().width < 768)
        await p
          .getByRole("button", { name: "Toggle navigation", exact: true })
          .click();
      await p
        .getByRole("navigation")
        .getByRole("button", { name: /^Load board/ })
        .click();
      await p.getByLabel("Search loads").fill(reference);
      await p.getByRole("button", { name: reference, exact: true }).click();
    };
    await openLoad(page);
    const review = page.getByRole("region", {
      name: "Native movement time review",
      exact: true,
    });
    await expect(review).toContainText("ORIGINAL-CARRIER-pickup");
    const fillReview = async (kind, minutes) => {
      await review
        .getByRole("button", { name: `Correct ${kind} time`, exact: true })
        .click();
      const form = page.getByRole("dialog", {
        name: `Correct carrier ${kind} time`,
        exact: true,
      });
      await form.getByLabel("Corrected actual time (UTC)").fill(past(minutes));
      await form
        .getByLabel("Actual carrier contact")
        .fill("PRIVATE actual carrier supervisor");
      await form
        .getByLabel("Movement correction reference")
        .fill(`PRIVATE-${kind}-REVIEW`);
      await form
        .getByLabel("Movement correction findings")
        .fill(
          "PRIVATE dispatcher and facility evidence reviewed against original carrier report",
        );
      await form.getByRole("checkbox").check();
      return form;
    };
    let form = await fillReview("pickup", 19);
    // A concurrent actual gate changes the load after this exact source review.
    await post(`loads/${load.id}/stop-events`, {
      expected_load_version: (await current()).version,
      stop: "pickup",
      kind: "arrival",
      local_time: past(25),
      timezone: "UTC",
      reference: "PRIVATE original gate receipt",
      note: "Actual origin gate evidence",
    });
    await form
      .getByRole("button", { name: "Save movement correction", exact: true })
      .click();
    await expect(form.getByRole("alert")).toContainText("Load changed");
    expect(
      (await snapshot()).movement_corrections.filter(
        (c) => c.load_id === load.id,
      ),
    ).toHaveLength(0);
    await page.keyboard.press("Escape");
    await openLoad(page);
    for (const [kind, minutes] of [
      ["pickup", 19],
      ["delivery", 4],
    ]) {
      form = await fillReview(kind, minutes);
      await form
        .getByRole("button", { name: "Save movement correction", exact: true })
        .click();
      await expect(form).toHaveCount(0);
      const card = review.getByRole("article", {
        name: `${kind} time source`,
        exact: true,
      });
      await card.locator("summary").click();
      await expect(card).toContainText(`PRIVATE-${kind}-REVIEW`);
      await expect(card).toContainText(
        "credential:" +
          createHash("sha256")
            .update("freight-test-operator-credential-at-least-32-chars")
            .digest("hex")
            .slice(0, 12),
      );
    }
    w = await snapshot();
    load = w.loads.find((l) => l.id === load.id);
    expect(physical(load)).toEqual(originalPhysical);
    expect(w.carrier_submissions.filter((s) => s.load_id === load.id)).toEqual(
      originals,
    );
    expect(
      w.movement_corrections.filter((c) => c.load_id === load.id),
    ).toHaveLength(2);
    expect(load.execution_timeline.consistent).toBe(true);
    expect(await (await page.request.get(rateUrl)).body()).toEqual(rateBytes);
    await page.setViewportSize({ width: 390, height: 844 });
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= window.innerWidth,
      ),
    ).toBe(true);
    if (process.env.FREIGHT_E2E_SCREENSHOTS === "true")
      await review.screenshot({
        path: "../docs/screenshots/movement-correction-mobile.png",
      });
    for (const [context, kind] of [
      [carrier, "carrier"],
      [customer, "customer"],
    ]) {
      const p = await context.newPage();
      p.on("pageerror", (e) => errors.push(e.message));
      await p.goto("/portal");
      const publicView = await (
        await context.request.get("/api/freight/portal")
      ).json();
      const timeline =
        kind === "carrier"
          ? publicView.offers.find((o) => o.id === offer.id).reporting
              .execution_timeline
          : publicView.shipments.find((l) => l.id === load.id)
              .execution_timeline;
      expect(JSON.stringify(timeline)).not.toContain("PRIVATE");
      expect(
        timeline.stops[0].facts.find((f) => f.kind === "pickup"),
      ).toMatchObject({
        corrected: true,
        original_observed_at: originals[0].observed_at,
      });
      const region = p
        .getByRole("article", {
          name:
            kind === "carrier"
              ? `Carrier offer ${reference} revision 1`
              : `Customer shipment ${reference}`,
          exact: true,
        })
        .getByRole("region", {
          name: "Shipment execution timeline",
          exact: true,
        });
      await region.locator("summary").click();
      await expect(region).toContainText("Corrected actual time");
      await expect(region).not.toContainText("PRIVATE");
      expect(
        await p.evaluate(
          () => document.documentElement.scrollWidth <= window.innerWidth,
        ),
      ).toBe(true);
    }
    const v = await viewer.newPage();
    await openLoad(v);
    await expect(
      v.getByRole("button", { name: "Correct pickup time", exact: true }),
    ).toBeDisabled();
    expect(
      (
        await viewer.request.post(
          `/api/freight/carrier-submissions/${originals[0].id}/time-corrections`,
          {
            headers: { Origin: origin, "Idempotency-Key": randomUUID() },
            data: {},
          },
        )
      ).status(),
    ).toBe(403);
    await post(`loads/${load.id}/commands`, {
      action: "attach_pod",
      expected_version: load.version,
      pod_reference: "Actual signed consolidated delivery receipt",
    });
    await post(`loads/${load.id}/pod/review`, {
      expected_version: (await current()).version,
      decision: "accepted",
      note: "Actual original receiver evidence reviewed",
    });
    await post("bills", {
      load_id: load.id,
      number: reference + "-BILL",
      due_date: day,
    });
    await openLoad(page);
    await expect(
      review.getByRole("button", { name: "Correct pickup time", exact: true }),
    ).toBeDisabled();
    await expect(
      review.getByText("Movement evidence is locked by financial records."),
    ).toHaveCount(2);
    expect(errors).toEqual([]);
  } finally {
    await Promise.allSettled([
      carrier.close(),
      customer.close(),
      viewer.close(),
    ]);
  }
});
