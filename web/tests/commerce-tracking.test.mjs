import test from "node:test";
import assert from "node:assert/strict";
import { trackingIntent } from "../lib/commerce-tracking.mjs";
import { originalValid } from "../lib/save-intent.mjs";
import {
  sealJournalIntent,
  openJournalIntent,
} from "../lib/save-journal-crypto.mjs";
const order = "00000000-0000-4000-8000-000000000001";
const shipment = "00000000-0000-4000-8000-000000000005";
const load = {
  id: "load_native",
  sync_order_id: order,
  version: 7,
  status: "delivered",
  source_order: { sourceSystem: "icommerce" },
};
const preview = {
  load_id: load.id,
  expected_load_version: 7,
  shipment: { id: shipment, order_id: order, version: 3 },
  plan: {
    shipment_id: shipment,
    order_id: order,
    expected_version: 3,
    next_status: "in_transit",
  },
};
test("shipment review freezes only original load/native versions and remains an encrypted recoverable save", () => {
  const intent = trackingIntent(load, preview);
  assert.deepEqual(intent, {
    path: "loads/load_native/commerce-tracking",
    body: {
      expected_load_version: 7,
      shipment_id: shipment,
      expected_shipment_version: 3,
    },
  });
  const attempt = {
    ...intent,
    key: "original-native-intent",
    message: "Shipment step queued",
    kind: "unknown",
    checks: 0,
    createdAt: "2026-10-19T00:00:00Z",
    fingerprint: JSON.stringify([intent.path, intent.body]),
  };
  assert.equal(originalValid(attempt), true);
  const owner = JSON.stringify(["credential", "", "demo", "user:ops", ""]);
  const env = {
    FREIGHT_SAVE_JOURNAL_KEYS_JSON: JSON.stringify({
      active: "owned",
      keys: { owned: Buffer.alloc(32, 7).toString("base64url") },
    }),
  };
  const payload = Object.fromEntries(
    ["key", "path", "body", "message", "createdAt"].map((key) => [
      key,
      attempt[key],
    ]),
  );
  const sealed = sealJournalIntent(owner, payload, env);
  assert.equal(JSON.stringify(sealed).includes(shipment), false);
  assert.deepEqual(openJournalIntent(owner, sealed, env), payload);
  assert.throws(() => openJournalIntent(owner + "different", sealed, env));
  preview.shipment.version = 99;
  assert.equal(intent.body.expected_shipment_version, 3);
  preview.shipment.version = 3;
});
test("changed load, order, shipment or reviewed version cannot produce a new save", () => {
  for (const mutate of [
    (v) => (v.load_id = "other"),
    (v) => v.expected_load_version++,
    (v) => (v.shipment.order_id = "other"),
    (v) => (v.shipment.id = "other"),
    (v) => v.shipment.version++,
    (v) => (v.plan.shipment_id = "other"),
    (v) => (v.plan.next_status = "cancelled"),
  ]) {
    const changed = structuredClone(preview);
    mutate(changed);
    assert.throws(() => trackingIntent(load, changed));
  }
  assert.throws(() => trackingIntent({ ...load, status: "planned" }, preview));
  assert.throws(() =>
    trackingIntent(
      { ...load, source_order: { sourceSystem: "sync" } },
      preview,
    ),
  );
});
