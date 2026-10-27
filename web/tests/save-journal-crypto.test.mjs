import { createHash } from "node:crypto";
import test from "node:test";
import assert from "node:assert/strict";
import {
  sealJournalIntent,
  openJournalIntent,
  journalKeyring,
} from "../lib/save-journal-crypto.mjs";
const owner = JSON.stringify(["credential", "", "tenant_a", "operator_a", ""]);
const intent = {
  key: "owned-uuid-key",
  path: "customers",
  body: { name: "PRIVATE freight customer", contact: "private@example.test" },
  message: "Customer added",
  createdAt: "2026-10-16T12:00:00.000Z",
};
const oldKey = Buffer.alloc(32, 3).toString("base64url"),
  newKey = Buffer.alloc(32, 9).toString("base64url");
const env = {
  FREIGHT_SAVE_JOURNAL_KEYS_JSON: JSON.stringify({
    active: "old",
    keys: { old: oldKey },
  }),
};
test("authenticated random envelopes hide original values and bind the complete verified owner", () => {
  const a = sealJournalIntent(owner, intent, env),
    b = sealJournalIntent(owner, intent, env);
  assert.notEqual(a.iv, b.iv);
  assert.notEqual(a.ciphertext, b.ciphertext);
  assert.equal(JSON.stringify(a).includes("PRIVATE"), false);
  assert.equal(JSON.stringify(a).includes("private@example.test"), false);
  assert.deepEqual(openJournalIntent(owner, a, env), intent);
  for (const scope of [
    ["credential", "", "tenant_b", "operator_a", ""],
    ["credential", "", "tenant_a", "operator_b", ""],
    ["clerk", "org_a", "tenant_a", "operator_a", "sess_a"],
  ])
    assert.throws(() => openJournalIntent(JSON.stringify(scope), a, env), {
      code: "save_journal_unreadable",
    });
});
test("tampering, truncated tags, malformed base64 and key-id substitution never return plaintext", () => {
  const a = sealJournalIntent(owner, intent, env);
  for (const changed of [
    { ...a, tag: a.tag.slice(1) },
    {
      ...a,
      ciphertext: (a.ciphertext[0] === "A" ? "B" : "A") + a.ciphertext.slice(1),
    },
    { ...a, iv: "!".repeat(16) },
    { ...a, version: 99 },
    { ...a, extra: "ignored?" },
  ])
    assert.throws(() => openJournalIntent(owner, changed, env), {
      code: "save_journal_unreadable",
    });
  const alias = {
    FREIGHT_SAVE_JOURNAL_KEYS_JSON: JSON.stringify({
      active: "other",
      keys: { old: oldKey, other: oldKey },
    }),
  };
  assert.throws(
    () => openJournalIntent(owner, { ...a, key_id: "other" }, alias),
    { code: "save_journal_unreadable" },
  );
});
test("key rotation retains original envelopes and missing keys remain recoverable failures", () => {
  const a = sealJournalIntent(owner, intent, env),
    rotation = {
      FREIGHT_SAVE_JOURNAL_KEYS_JSON: JSON.stringify({
        active: "new",
        keys: { old: oldKey, new: newKey },
      }),
    };
  assert.deepEqual(openJournalIntent(owner, a, rotation), intent);
  assert.equal(sealJournalIntent(owner, intent, rotation).key_id, "new");
  assert.throws(
    () =>
      openJournalIntent(owner, a, {
        FREIGHT_SAVE_JOURNAL_KEYS_JSON: JSON.stringify({
          active: "new",
          keys: { new: newKey },
        }),
      }),
    { code: "save_journal_key_unavailable" },
  );
});
test("journal keys are domain separated from cookie keys and malformed configuration fails closed", () => {
  const fallback = {
    FREIGHT_SESSION_SECRET:
      "owned-credential-session-secret-at-least-32-characters",
  };
  assert.deepEqual(
    openJournalIntent(
      owner,
      sealJournalIntent(owner, intent, fallback),
      fallback,
    ),
    intent,
  );
  assert.notDeepEqual(
    journalKeyring(fallback).keys.get("session-v1"),
    createHash("sha256").update(fallback.FREIGHT_SESSION_SECRET).digest(),
  );
  assert.deepEqual(
    journalKeyring({
      ...fallback,
      FREIGHT_SAVE_JOURNAL_KEYS_JSON: "",
    }).keys.get("session-v1"),
    journalKeyring(fallback).keys.get("session-v1"),
  );
  for (const e of [
    {},
    { FREIGHT_SESSION_SECRET: "short" },
    { FREIGHT_SAVE_JOURNAL_KEYS_JSON: "bad" },
    {
      FREIGHT_SAVE_JOURNAL_KEYS_JSON: JSON.stringify({
        active: "a",
        keys: { a: "wrong" },
      }),
    },
  ])
    assert.throws(() => journalKeyring(e), {
      code: "save_journal_key_unavailable",
    });
  for (const bad of [
    { ...intent, actor: "admin" },
    { ...intent, path: "../customers" },
    { ...intent, body: { name: "x".repeat(1024 * 1024) } },
  ])
    assert.throws(() => sealJournalIntent(owner, bad, env), {
      code: "invalid_save_journal",
    });
});

test("the secret-free synthetic demo uses a random process key while real and Clerk workspaces fail closed", () => {
  const demo = { FREIGHT_DEMO: "true" },
    a = sealJournalIntent(owner, intent, demo);
  assert.match(a.key_id, /^demo-/);
  assert.deepEqual(openJournalIntent(owner, a, demo), intent);
  assert.equal(sealJournalIntent(owner, intent, demo).key_id, a.key_id);
  for (const config of [
    { FREIGHT_DEMO: "false" },
    { FREIGHT_DEMO: "true", FREIGHT_WEB_AUTH_MODE: "clerk" },
    { FREIGHT_DEMO: "true", FREIGHT_SESSION_SECRET: "short" },
  ])
    assert.throws(() => sealJournalIntent(owner, intent, config), {
      code: "save_journal_key_unavailable",
    });
});

test("invoice entry reviews and independent approvals retain their exact encrypted original revision and findings", () => {
  for (const [action, body] of [
    [
      "entry-review",
      {
        expected_revision: "a".repeat(64),
        evidence_reference: "PRIVATE carrier source INV-1180",
        note: "Actual carrier invoice and agreed charges inspected",
      },
    ],
    [
      "approve",
      {
        expected_revision: "a".repeat(64),
        note: "Independent delivery and invoice evidence verified",
      },
    ],
    // Historical empty-body approvals remain recoverable after the upgrade.
    ["approve", {}],
  ]) {
    const original = {
      ...intent,
      path: `invoices/invoice_1180/${action}`,
      body,
    };
    const sealed = sealJournalIntent(owner, original, env);
    assert.deepEqual(openJournalIntent(owner, sealed, env), original);
    assert.equal(JSON.stringify(sealed).includes("PRIVATE"), false);
    assert.throws(
      () =>
        openJournalIntent(
          JSON.stringify([
            "credential",
            "",
            "tenant_a",
            "another_finance_user",
            "",
          ]),
          sealed,
          env,
        ),
      { code: "save_journal_unreadable" },
    );
  }
});
