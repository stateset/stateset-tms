import test from "node:test";
import assert from "node:assert/strict";
import { externalReviewIntent } from "../lib/commerce-review.mjs";
import { originalValid } from "../lib/save-intent.mjs";
import {
  sealJournalIntent,
  openJournalIntent,
} from "../lib/save-journal-crypto.mjs";
const shipment = "00000000-0000-4000-8000-000000000005";
const order = "00000000-0000-4000-8000-000000000001";
const load = { id: "load_native", version: 9, sync_order_id: order };
const job = {
  id: "outbox_native",
  load_id: load.id,
  destination: "stateset-icommerce",
  status: "held",
  total_attempts: 2,
  payload: { plan: { shipment_id: shipment, expected_version: 3 } },
};
const preview = {
  job_id: job.id,
  load_id: load.id,
  expected_load_version: 9,
  expected_total_attempts: 2,
  late_original_fenced: true,
  original_plan: { expected_version: 3 },
  shipment: { id: shipment, order_id: order, version: 7 },
};
const fields = {
  contact: "Shipping operator",
  reference: "EXTERNAL-1",
  note: "Reviewed native activity; original receipt unavailable",
};
test("external review freezes only caller intent and uses the owner-bound encrypted save journal", () => {
  const intent = externalReviewIntent(job, load, preview, fields);
  assert.deepEqual(intent, {
    path: "outbox/outbox_native/commerce-review",
    body: {
      expected_load_version: 9,
      expected_total_attempts: 2,
      expected_shipment_version: 7,
      ...fields,
    },
  });
  const retained = {
    ...intent,
    key: "original-external-review",
    message: "Review recorded",
    createdAt: "2026-10-19T00:00:00Z",
  };
  const attempt = {
    ...retained,
    kind: "unknown",
    checks: 0,
    fingerprint: JSON.stringify([intent.path, intent.body]),
  };
  assert.equal(originalValid(attempt), true);
  assert.equal(
    originalValid({
      ...attempt,
      path: "inbox/outbox_native/commerce-review",
      fingerprint: JSON.stringify([
        "inbox/outbox_native/commerce-review",
        intent.body,
      ]),
    }),
    false,
  );
  const env = {
    FREIGHT_SAVE_JOURNAL_KEYS_JSON: JSON.stringify({
      active: "owned",
      keys: { owned: Buffer.alloc(32, 9).toString("base64url") },
    }),
  };
  const owner = JSON.stringify([
    "clerk",
    "org_1",
    "tenant_1",
    "user_ops",
    "session_1",
  ]);
  const encrypted = sealJournalIntent(owner, retained, env);
  assert.equal(JSON.stringify(encrypted).includes(fields.reference), false);
  assert.deepEqual(openJournalIntent(owner, encrypted, env), retained);
  assert.throws(() =>
    openJournalIntent(owner.replace("session_1", "session_2"), encrypted, env),
  );
  const mutable = structuredClone(preview);
  const frozen = externalReviewIntent(job, load, mutable, fields);
  mutable.shipment.version++;
  assert.equal(frozen.body.expected_shipment_version, 7);
});
test("review refuses stale local identity, attempts, native versions, leases and missing fencing", () => {
  for (const mutate of [
    (j) => {
      delete j.id;
    },
    (j, l) => {
      delete l.id;
    },
    (j, l, p) => {
      delete l.sync_order_id;
      delete p.shipment.order_id;
    },
    (j, l, p) => {
      j.id = "other";
    },
    (j, l, p) => {
      l.id = "other";
    },
    (j, l, p) => {
      l.version++;
    },
    (j, l, p) => {
      j.total_attempts++;
    },
    (j, l, p) => {
      p.shipment.id = "other";
    },
    (j, l, p) => {
      p.shipment.order_id = "other";
    },
    (j, l, p) => {
      p.shipment.version = 3;
    },
    (j, l, p) => {
      p.shipment.version = 2147483648;
    },
    (j, l, p) => {
      p.late_original_fenced = false;
    },
    (j, l, p) => {
      j.lease = "active";
    },
    (j, l, p) => {
      j.leased_until = "2026-10-19T00:00:00Z";
    },
    (j, l, p) => {
      j.status = "external_reviewed";
    },
    (j, l, p) => {
      j.destination = "stateset-sync-server";
    },
    (j, l, p) => {
      l.version = p.expected_load_version = Number.MAX_SAFE_INTEGER + 1;
    },
  ]) {
    const j = structuredClone(job),
      l = structuredClone(load),
      p = structuredClone(preview);
    mutate(j, l, p);
    assert.throws(() => externalReviewIntent(j, l, p, fields));
  }
  for (const value of [
    "",
    "  ",
    "new\nline",
    "x".repeat(201),
    "é".repeat(101),
    null,
  ]) {
    assert.throws(() =>
      externalReviewIntent(job, load, preview, { ...fields, note: value }),
    );
  }
});
