const { test, expect } = require("@playwright/test");
const { randomUUID, createHash } = require("node:crypto");

test("four-stop planning and carrier execution preserve ordered evidence and exact full-route agreement", async ({
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
  await page.getByRole("button", { name: "Open workspace" }).click();
  await expect(
    page.getByRole("heading", { name: "Overview", exact: true }),
  ).toBeVisible();
  const origin = new URL(page.url()).origin,
    reference = `ROUTE-${Date.now()}`,
    day = new Date(Date.now() + 86400000).toISOString().slice(0, 10);
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
  await page.getByRole("button", { name: "Create load", exact: true }).click();
  let d = page.getByRole("dialog");
  for (const [name, value] of Object.entries({
    reference,
    commodity: "Full route freight",
    origin_facility: "Portland origin",
    origin_city: "Portland",
    origin_state: "OR",
    origin_address: "15 Dock Street",
    origin_postal: "97201",
    pickup: day,
    destination_facility: "Seattle final receiver",
    destination_city: "Seattle",
    destination_state: "WA",
    destination_address: "25 Receiving Ave",
    destination_postal: "98101",
    delivery: day,
    buy: "1250.01",
    sell: "1500.01",
    miles: "173",
  }))
    await d.locator(`[name="${name}"]`).fill(value);
  await d.getByLabel("Customer", { exact: true }).selectOption("c1");
  for (const [i, facility, city, zip, kind] of [
    [2, "Tacoma intermediate receiver", "Tacoma", "98402", "delivery"],
    [3, "Kent additional pickup", "Kent", "98032", "pickup"],
  ]) {
    await d
      .getByRole("button", { name: "Add intermediate stop", exact: true })
      .click();
    await d.getByLabel(`Stop ${i} service`, { exact: true }).selectOption(kind);
    for (const [label, value] of [
      ["facility", facility],
      ["street address", `${i} Owned Route Street`],
      ["city", city],
      ["state/province", "WA"],
      ["postal code", zip],
      ["planned date", day],
      ["contact", `Actual stop ${i} lead`],
      ["instructions", `Actual stop ${i} handling instructions`],
    ])
      await d.getByLabel(`Stop ${i} ${label}`, { exact: true }).fill(value);
  }
  await d
    .getByRole("button", { name: "Move stop 3 earlier", exact: true })
    .click();
  await expect(d.getByLabel("Stop 2 facility", { exact: true })).toHaveValue(
    "Kent additional pickup",
  );
  await d
    .getByRole("button", { name: "Move stop 2 later", exact: true })
    .click();
  await expect(d.getByLabel("Stop 2 facility", { exact: true })).toHaveValue(
    "Tacoma intermediate receiver",
  );
  await d.getByRole("button", { name: "Create load", exact: true }).click();
  await expect(d).toHaveCount(0);
  let w = await snapshot(),
    load = w.loads.find((l) => l.reference === reference);
  expect(load.intermediate_stops.map((s) => s.stop.facility)).toEqual([
    "Tacoma intermediate receiver",
    "Kent additional pickup",
  ]);
  await post(`loads/${load.id}/commands`, {
    action: "tender",
    expected_version: load.version,
    carrier_id: "ca4",
  });
  w = await snapshot();
  load = w.loads.find((l) => l.id === load.id);
  expect(
    w.outbox.find(
      (j) => j.load_id === load.id && j.destination === "stateset-edi",
    ),
  ).toMatchObject({ status: "native_only", total_attempts: 0 });
  const rate = await post(`loads/${load.id}/rate-confirmations`, {
    expected_load_version: load.version,
    terms: "Actual inclusive full four-stop carrier agreement",
    reason: "Full route published to assigned carrier",
  });
  w = await snapshot();
  load = w.loads.find((l) => l.id === load.id);
  const offer = await post(`loads/${load.id}/portal-offers`, {
    expected_load_version: load.version,
    rate_confirmation_id: rate.id,
  });
  const original = await page.request.get(
      `/api/freight/rate-confirmations/${rate.id}/statement`,
    ),
    bytes = await original.body();
  expect(createHash("sha256").update(bytes).digest("hex")).toBe(rate.sha256);
  expect(bytes.toString()).toContain("Tacoma intermediate receiver");
  expect(bytes.toString()).toContain("Kent additional pickup");
  const context = await browser.newContext({
      viewport: { width: 390, height: 844 },
    }),
    p = await context.newPage();
  p.setDefaultTimeout(12000);
  p.on("pageerror", (e) => errors.push(e.message));
  try {
    await p.goto(`${origin}/portal`);
    await p
      .getByLabel("Partner credential")
      .fill("freight-test-carrier-credential-at-least-32-chars");
    await p.getByRole("button", { name: "Open partner portal" }).click();
    const card = p.getByRole("article", {
      name: `Carrier offer ${reference} revision 1`,
      exact: true,
    });
    await expect(
      card.getByRole("heading", { name: "Stop 2 · delivery", exact: true }),
    ).toBeVisible();
    await expect(
      card.getByRole("heading", { name: "Stop 3 · pickup", exact: true }),
    ).toBeVisible();
    await card
      .getByRole("button", { name: "Accept carrier offer", exact: true })
      .click();
    d = p.getByRole("dialog");
    await d.getByRole("checkbox").check();
    await d
      .getByLabel("Your response reference")
      .fill("Actual full route agreement");
    await d
      .getByLabel("Carrier response note")
      .fill("Dispatcher reviewed all four stops and inclusive rate");
    await d
      .getByRole("button", { name: "Record carrier response", exact: true })
      .click();
    await expect(d).toHaveCount(0);
    const observe = async (kind, minutes, location) => {
      await card
        .getByRole("button", { name: "Report shipment progress", exact: true })
        .click();
      const form = p.getByRole("dialog");
      await form.getByLabel("Shipment observation").selectOption(kind);
      await form
        .getByLabel("Observed time (UTC)")
        .fill(
          new Date(Date.now() - minutes * 60000).toISOString().slice(0, 16),
        );
      await form.getByLabel("Observed location").fill(location);
      await form
        .getByLabel("Evidence reference")
        .fill(`Actual ${kind} evidence`);
      await form
        .getByLabel("Carrier evidence note")
        .fill(`Actual physical ${kind} confirmed`);
      await form
        .getByRole("button", {
          name: "Record shipment observation",
          exact: true,
        })
        .click();
      await expect(form).toHaveCount(0);
    };
    await observe("pickup", 20, "Portland, OR");
    w = await snapshot();
    load = w.loads.find((l) => l.id === load.id);
    const prematureGate = await post(`loads/${load.id}/stop-events`, {
      expected_load_version: load.version,
      stop: load.intermediate_stops[1].id,
      kind: "arrival",
      local_time: new Date(Date.now() - 17 * 60000).toISOString().slice(0, 16),
      timezone: "UTC",
      reference: "PRIVATE-EXECUTION-GATE",
      note: "Original facility arrival evidence",
    });
    await p.reload();
    await expect(card).toBeVisible();
    await expect(
      card.getByRole("button", { name: "Confirm stop 3 service", exact: true }),
    ).toBeDisabled();
    await card
      .getByRole("button", { name: "Report shipment progress", exact: true })
      .click();
    await expect(
      p.getByLabel("Shipment observation").locator('option[value="delivery"]'),
    ).toHaveCount(0);
    await p.keyboard.press("Escape");
    // The fixture's events are minutes apart. Minute precision also avoids
    // Chromium canonicalizing fractional trailing zeros during Playwright fill.
    for (const [sequence, minutes] of [
      [2, 15],
      [3, 10],
    ]) {
      await card
        .getByRole("button", {
          name: `Confirm stop ${sequence} service`,
          exact: true,
        })
        .click();
      const form = p.getByRole("dialog");
      await form
        .getByLabel("Actual service time (UTC)")
        .fill(
          new Date(Date.now() - minutes * 60000).toISOString().slice(0, 16),
        );
      await form
        .getByLabel("Actual stop contact")
        .fill(`Actual facility ${sequence} lead`);
      await form
        .getByLabel("Stop service reference")
        .fill(`Actual stop ${sequence} service receipt`);
      await form
        .getByLabel("Stop service findings")
        .fill(
          "Carrier verified actual handling against receiver or shipping evidence",
        );
      await form.getByRole("checkbox").check();
      if (sequence === 2) {
        await form
          .getByRole("button", { name: "Save stop confirmation", exact: true })
          .click();
        await expect(form.getByRole("alert")).toContainText(
          "Execution chronology conflict",
        );
        w = await snapshot();
        expect(
          w.stop_services.filter((s) => s.load_id === load.id),
        ).toHaveLength(0);
        await post(`stop-events/${prematureGate.id}/correct`, {
          expected_load_version: w.loads.find((l) => l.id === load.id).version,
          local_time: new Date(Date.now() - 12 * 60000)
            .toISOString()
            .slice(0, 16),
          timezone: "UTC",
          reference: "PRIVATE-CORRECTED-GATE",
          note: "Actual facility time corrected; original retained",
        });
        await p.keyboard.press("Escape");
        await p.reload();
        await expect(card).toBeVisible();
        await card
          .getByRole("button", { name: "Confirm stop 2 service", exact: true })
          .click();
        await form
          .getByLabel("Actual service time (UTC)")
          .fill(new Date(Date.now() - 15 * 60000).toISOString().slice(0, 16));
        await form
          .getByLabel("Actual stop contact")
          .fill("Actual facility 2 lead");
        await form
          .getByLabel("Stop service reference")
          .fill("Actual corrected stop 2 receipt");
        await form
          .getByLabel("Stop service findings")
          .fill("Actual handling confirmed after gate evidence repair");
        await form.getByRole("checkbox").check();
      }
      let releaseRefresh,
        heldReads = 0;
      let holdRefresh;
      let refreshFulfilled;
      if (sequence === 3) {
        const barrier = new Promise((resolve) => {
          releaseRefresh = resolve;
        });
        let fulfilled;
        refreshFulfilled = new Promise((resolve) => {
          fulfilled = resolve;
        });
        holdRefresh = async (route) => {
          const response = await route.fetch();
          heldReads++;
          await barrier;
          await route.fulfill({ response });
          fulfilled();
        };
        await p.route("**/api/freight/portal", holdRefresh);
      }
      await form
        .getByRole("button", { name: "Save stop confirmation", exact: true })
        .click();
      if (sequence === 3) {
        await expect.poll(() => heldReads).toBeGreaterThan(0);
        await expect(form).toBeVisible();
        await expect(form.getByLabel("Actual stop contact")).toBeDisabled();
        releaseRefresh();
        await refreshFulfilled;
        await p.unroute("**/api/freight/portal", holdRefresh);
      }
      await expect(form).toHaveCount(0);
    }
    await observe("delivery", 5, "Seattle, WA");
    w = await snapshot();
    load = w.loads.find((l) => l.id === load.id);
    expect(load).toMatchObject({
      status: "delivered",
      pod_ready: false,
      route_attention: null,
    });
    expect(load.itinerary.all_intermediate_stops_confirmed).toBe(true);
    expect(w.stop_services.filter((s) => s.load_id === load.id)).toHaveLength(
      2,
    );
    expect(load.execution_timeline.consistent).toBe(true);
    expect(load.execution_timeline.record_count).toBe(5);
    const timeline = card.getByRole("region", {
      name: "Shipment execution timeline",
    });
    await timeline.locator("summary").click();
    await expect(timeline).toContainText("Gate arrival");
    await expect(timeline).toContainText("Unrecorded");
    await expect(timeline).not.toContainText("PRIVATE-EXECUTION-GATE");
    await expect(timeline).not.toContainText("PRIVATE-CORRECTED-GATE");
    expect(
      w.stop_services
        .filter((s) => s.load_id === load.id)
        .every(
          (s) => s.offer_id === offer.id && s.recorded_by === "test:carrier",
        ),
    ).toBe(true);
    const scoped = await p.request.get("/api/freight/portal");
    expect(
      (await scoped.json()).offers.find((o) => o.id === offer.id).reporting
        .itinerary.stops[1].service.contact,
    ).toBeUndefined();
    expect(
      await p.evaluate(
        () => document.documentElement.scrollWidth <= window.innerWidth,
      ),
    ).toBe(true);
    if (process.env.FREIGHT_E2E_SCREENSHOTS === "true")
      await card.screenshot({
        path: "../docs/screenshots/execution-timeline-mobile.png",
      });
    expect(
      await (
        await page.request.get(
          `/api/freight/rate-confirmations/${rate.id}/statement`,
        )
      ).body(),
    ).toEqual(bytes);
    expect(errors).toEqual([]);
  } finally {
    await context.close();
  }
});
