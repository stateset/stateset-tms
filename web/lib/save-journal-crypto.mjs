import {
  createCipheriv,
  createDecipheriv,
  createHash,
  randomBytes,
} from "node:crypto";
import { originalValid } from "./save-intent.mjs";
const purpose = "stateset-freight/save-journal/v1";
let demoKeyring;
const fields = ["key", "path", "body", "message", "createdAt"];
const failure = (message, status, code) =>
  Object.assign(new Error(message), { status, code });
const unreadable = () =>
  failure(
    "Saved recovery data could not be verified. Retain this browser's data for support.",
    409,
    "save_journal_unreadable",
  );
export function journalKeyring(env = process.env) {
  try {
    if (
      env.FREIGHT_SAVE_JOURNAL_KEYS_JSON !== undefined &&
      env.FREIGHT_SAVE_JOURNAL_KEYS_JSON !== ""
    ) {
      const ring = JSON.parse(env.FREIGHT_SAVE_JOURNAL_KEYS_JSON);
      if (
        Object.keys(ring).sort().join() !== "active,keys" ||
        !/^[A-Za-z0-9_-]{1,64}$/.test(ring.active) ||
        typeof ring.active !== "string" ||
        !ring.keys ||
        Array.isArray(ring.keys) ||
        Object.keys(ring.keys).length > 10 ||
        !Object.hasOwn(ring.keys, ring.active)
      )
        throw new Error();
      const keys = new Map();
      for (const [name, value] of Object.entries(ring.keys)) {
        if (
          !/^[A-Za-z0-9_-]{1,64}$/.test(name) ||
          typeof value !== "string" ||
          !/^[A-Za-z0-9_-]{43}$/.test(value)
        )
          throw new Error();
        const bytes = Buffer.from(value, "base64url");
        if (bytes.length !== 32 || bytes.toString("base64url") !== value)
          throw new Error();
        keys.set(name, bytes);
      }
      return { active: ring.active, keys };
    }
    const secret = env.FREIGHT_SESSION_SECRET;
    if (
      (secret === undefined || secret === "") &&
      env.FREIGHT_DEMO === "true" &&
      (env.FREIGHT_WEB_AUTH_MODE || "credential") === "credential"
    ) {
      if (!demoKeyring) {
        const active = "demo-" + randomBytes(8).toString("hex");
        demoKeyring = { active, keys: new Map([[active, randomBytes(32)]]) };
      }
      return demoKeyring;
    }
    if (typeof secret !== "string" || secret.length < 32) throw new Error();
    return {
      active: "session-v1",
      keys: new Map([
        [
          "session-v1",
          createHash("sha256")
            .update(purpose + "\0" + secret)
            .digest(),
        ],
      ]),
    };
  } catch {
    throw failure(
      "Recovery encryption is unavailable. Check the server's journal key configuration.",
      503,
      "save_journal_key_unavailable",
    );
  }
}
export function validateIntent(intent) {
  if (
    !intent ||
    Object.keys(intent).sort().join() !== [...fields].sort().join() ||
    !originalValid({
      ...intent,
      fingerprint: JSON.stringify([intent.path, intent.body]),
      kind: "unknown",
      checks: 0,
    })
  )
    throw failure(
      "Invalid original save for recovery.",
      400,
      "invalid_save_journal",
    );
  return intent;
}
function aad(owner, keyId) {
  return Buffer.from(JSON.stringify([purpose, keyId, owner]));
}
export function sealJournalIntent(owner, input, env = process.env) {
  const intent = validateIntent(input),
    ring = journalKeyring(env),
    iv = randomBytes(12);
  const cipher = createCipheriv("aes-256-gcm", ring.keys.get(ring.active), iv, {
    authTagLength: 16,
  });
  cipher.setAAD(aad(owner, ring.active));
  const encrypted = Buffer.concat([
    cipher.update(JSON.stringify(intent), "utf8"),
    cipher.final(),
  ]);
  return {
    version: 1,
    key_id: ring.active,
    iv: iv.toString("base64url"),
    tag: cipher.getAuthTag().toString("base64url"),
    ciphertext: encrypted.toString("base64url"),
  };
}
function bytes(value, length, max = length) {
  if (
    typeof value !== "string" ||
    value.length > Math.ceil((max * 4) / 3) ||
    !/^[A-Za-z0-9_-]+$/.test(value)
  )
    throw unreadable();
  const result = Buffer.from(value, "base64url");
  if (
    result.length < length ||
    result.length > max ||
    result.toString("base64url") !== value
  )
    throw unreadable();
  return result;
}
export function openJournalIntent(owner, sealed, env = process.env) {
  if (
    !sealed ||
    Object.keys(sealed).sort().join() !== "ciphertext,iv,key_id,tag,version" ||
    sealed.version !== 1 ||
    typeof sealed.key_id !== "string" ||
    !/^[A-Za-z0-9_-]{1,64}$/.test(sealed.key_id)
  )
    throw unreadable();
  const ring = journalKeyring(env),
    key = ring.keys.get(sealed.key_id);
  if (!key)
    throw failure(
      "A retained save needs an unavailable journal key. Restore that key and retry recovery.",
      503,
      "save_journal_key_unavailable",
    );
  try {
    const decipher = createDecipheriv(
      "aes-256-gcm",
      key,
      bytes(sealed.iv, 12),
      { authTagLength: 16 },
    );
    decipher.setAAD(aad(owner, sealed.key_id));
    decipher.setAuthTag(bytes(sealed.tag, 16));
    const plaintext = Buffer.concat([
      decipher.update(bytes(sealed.ciphertext, 1, 1024 * 1024 + 16384)),
      decipher.final(),
    ]);
    // No unauthenticated bytes are returned before final() verifies the tag.
    return validateIntent(JSON.parse(plaintext.toString("utf8")));
  } catch {
    throw unreadable();
  }
}
