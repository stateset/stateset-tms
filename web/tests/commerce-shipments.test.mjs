import test from "node:test";
import assert from "node:assert/strict";
import {
  validateShipmentPage,
  candidateShipmentId,
  MAX_SHIPMENT_PAGE,
} from "../lib/commerce-shipments.mjs";
const order = "00000000-0000-4000-8000-000000000001";
const shipment = "00000000-0000-4000-8000-000000000005";
const load = {
  id: "load_native",
  sync_order_id: order,
  version: 7,
  status: "delivered",
  source_order: { sourceSystem: "icommerce" },
};
const row = {
  shipment_number: "NATIVE-1",
  state: "ready_for_review",
  reason: "Review shipped → in_transit",
  shipment: {
    id: shipment,
    order_id: order,
    version: 3,
    status: "shipped",
    items: [],
  },
  plan: {
    shipment_id: shipment,
    order_id: order,
    expected_version: 3,
    from_status: "shipped",
    next_status: "in_transit",
    target_status: "delivered",
  },
};
const page = {
  load_id: load.id,
  order_id: order,
  expected_load_version: 7,
  page: 1,
  page_size: 25,
  total: 1,
  has_more: false,
  active_handoffs: [],
  items: [row],
};
test("native candidate selection returns only identity for a fresh individual preview", () => {
  assert.equal(validateShipmentPage(load, page, 1), page);
  assert.equal(candidateShipmentId(load, page, shipment), shipment);
  assert.equal(typeof candidateShipmentId(load, page, shipment), "string");
  const external = structuredClone(page);
  external.items[0].shipment.version = 9;
  external.items[0].plan.expected_version = 9;
  // Discovery never freezes a write body or substitutes an old native version.
  assert.equal(candidateShipmentId(load, external, shipment), shipment);
});
test("wrong owner context, stale load, foreign order, malformed rows and pages refuse selection", () => {
  for (const mutate of [
    (p) => (p.load_id = "other"),
    (p) => (p.order_id = "other"),
    (p) => p.expected_load_version++,
    (p) => p.page++,
    (p) => (p.page_size = 100),
    (p) => (p.has_more = true),
    (p) => delete p.has_more,
    (p) => p.items.push(structuredClone(p.items[0])),
    (p) => (p.items[0].shipment.order_id = "other"),
    (p) => (p.items[0].shipment.id = "invalid"),
    (p) => (p.items[0].shipment.version = -1),
    (p) => (p.items[0].shipment.version = 2147483648),
    (p) => (p.items[0].shipment.status = "shipped\n"),
    (p) => (p.items[0].shipment_number = ""),
    (p) => (p.items[0].state = "approved"),
    (p) => p.items[0].plan.expected_version++,
    (p) => (p.active_handoffs = [{ id: "other", status: "synced" }]),
  ]) {
    const altered = structuredClone(page);
    mutate(altered);
    assert.throws(() => validateShipmentPage(load, altered, 1));
  }
  assert.throws(() => validateShipmentPage({ ...load, version: 8 }, page, 1));
  assert.throws(() =>
    validateShipmentPage(
      { ...load, source_order: { sourceSystem: "sync" } },
      page,
      1,
    ),
  );
  assert.throws(() =>
    validateShipmentPage(
      { ...load, version: Number.MAX_SAFE_INTEGER + 1 },
      { ...page, expected_load_version: Number.MAX_SAFE_INTEGER + 1 },
      1,
    ),
  );
  assert.throws(() =>
    candidateShipmentId(load, page, "00000000-0000-4000-8000-000000000099"),
  );
});
test("blocked, already current and outstanding handoffs cannot be selected for another step", () => {
  for (const state of ["blocked", "up_to_date"]) {
    const current = structuredClone(page);
    current.items[0].state = state;
    current.items[0].plan = null;
    assert.equal(validateShipmentPage(load, current, 1), current);
    assert.throws(() => candidateShipmentId(load, current, shipment));
  }
  const active = structuredClone(page);
  active.active_handoffs = [{ id: "outbox_original", status: "held" }];
  assert.throws(() => candidateShipmentId(load, active, shipment));
  active.items[0].state = "blocked";
  assert.equal(validateShipmentPage(load, active, 1), active);
  assert.throws(() => candidateShipmentId(load, active, shipment));
});
test("native pagination allows empty last pages and full live pages within the server offset range", () => {
  const full = structuredClone(page);
  full.items = Array.from({ length: 25 }, (_, n) => {
    const r = structuredClone(row);
    r.shipment.id = `00000000-0000-4000-8000-${(500 + n).toString(16).padStart(12, "0")}`;
    r.plan.shipment_id = r.shipment.id;
    return r;
  });
  full.has_more = true;
  assert.equal(validateShipmentPage(load, full, 1), full);
  const last = { ...page, page: 2, has_more: false, items: [] };
  assert.equal(validateShipmentPage(load, last, 2), last);
  const end = { ...last, page: MAX_SHIPMENT_PAGE };
  assert.equal(validateShipmentPage(load, end, MAX_SHIPMENT_PAGE), end);
  assert.throws(() =>
    validateShipmentPage(
      load,
      { ...end, page: MAX_SHIPMENT_PAGE + 1 },
      MAX_SHIPMENT_PAGE + 1,
    ),
  );
  assert.throws(() => validateShipmentPage(load, page, 0));
});
