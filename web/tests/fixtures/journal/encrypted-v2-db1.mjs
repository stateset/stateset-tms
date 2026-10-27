import { originalValid } from "./save-intent.mjs";
// One original per verified owner in this browser. IndexedDB readwrite
// transactions serialize claim/decision/clear across pages; HTTP is never retried
// by this module. A sign-out generation fences even a delayed first claim.
export const JOURNAL_DATABASE = "stateset-freight-save-recovery";
const STORE = "owners";
export const JOURNAL_CHANNEL = "stateset-freight-save-recovery-v1";
const unavailable = () =>
  new Error(
    "Browser save recovery is unavailable. New shared-workspace saves are paused. Retry recovery storage.",
  );
export function journalOwner(access, mode, organization = "", session = "") {
  if (!access?.tenant_id || !access?.access?.subject)
    throw new Error("The workspace identity could not be verified.");
  return JSON.stringify([
    mode,
    organization || "",
    access.tenant_id,
    access.access.subject,
    session || "",
  ]);
}
export function expectedAccess(owner) {
  const [, , tenant_id, subject] = JSON.parse(owner);
  return encodeURIComponent(JSON.stringify({ tenant_id, subject }));
}
function unreadable() {
  return new Error(
    "The saved recovery record cannot be read safely. Shared-workspace saves are paused; retain this browser's data for support.",
  );
}
function sealedValid(value) {
  return (
    value &&
    Object.keys(value).sort().join() === "ciphertext,iv,key_id,tag,version" &&
    value.version === 1 &&
    typeof value.key_id === "string" &&
    /^[A-Za-z0-9_-]{1,64}$/.test(value.key_id) &&
    typeof value.iv === "string" &&
    /^[A-Za-z0-9_-]{16}$/.test(value.iv) &&
    typeof value.tag === "string" &&
    /^[A-Za-z0-9_-]{22}$/.test(value.tag) &&
    typeof value.ciphertext === "string" &&
    value.ciphertext.length <= Math.ceil(((1024 * 1024 + 16384) * 4) / 3) &&
    /^[A-Za-z0-9_-]+$/.test(value.ciphertext)
  );
}
function validate(record) {
  const a = record?.attempt;
  if (
    !record ||
    record.version !== 2 ||
    Object.keys(record).sort().join() !== "attempt,generation,version" ||
    !/^[a-f0-9-]{36}$/.test(record.generation) ||
    (a !== null &&
      (!a ||
        Object.keys(a).sort().join() !== "checks,key,kind,sealed" ||
        !/^[\x20-\x7e]{1,200}$/.test(a.key) ||
        !["unknown", "confirmed", "stopped"].includes(a.kind) ||
        !Number.isSafeInteger(a.checks) ||
        a.checks < 0 ||
        !sealedValid(a.sealed)))
  )
    throw unreadable();
  return record;
}
function intent(attempt) {
  if (!originalValid(attempt)) throw unreadable();
  const { key, path, body, message, createdAt } = attempt;
  return { key, path, body, message, createdAt };
}
function packed(attempt, sealed) {
  return {
    key: attempt.key,
    kind: attempt.kind,
    checks: attempt.checks,
    sealed,
  };
}
async function defaultCodec(action, owner, value) {
  const response = await fetch("/api/freight/save-journal", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      action,
      owner,
      [action === "seal" ? "intent" : "sealed"]: value,
    }),
    signal: AbortSignal.timeout(15000),
  });
  const result = await response.json();
  if (!response.ok)
    throw Object.assign(
      new Error(result.error?.message || "Save recovery is unavailable."),
      { status: response.status, code: result.error?.code },
    );
  return result.data;
}
// Network encryption/decryption stays outside IndexedDB transactions. Only
// ciphertext and non-business coordination metadata are ever written as v2.
export async function saveJournal(
  owner,
  operation,
  options = {},
  factory = globalThis.indexedDB,
) {
  const codec = options.codec || defaultCodec;
  let snapshot, stored;
  if (operation === "claim") {
    snapshot = JSON.parse(JSON.stringify(options.attempt));
    stored = packed(snapshot, await codec("seal", owner, intent(snapshot)));
  }
  for (let retry = 0; retry < 3; retry++) {
    const record = await journalTransaction(
      owner,
      operation,
      { ...options, ...(stored ? { attempt: stored } : {}) },
      factory,
    );
    if (operation === "forget")
      return {
        version: record.version,
        generation: record.generation,
        attempt: null,
      };
    options.onGeneration?.(record.generation);
    if (record.legacy) {
      const original = record.attempt;
      if (!originalValid(original)) throw unreadable();
      const sealed = await codec("seal", owner, intent(original));
      await journalTransaction(
        owner,
        "upgrade",
        {
          expected: record.expected,
          attempt: packed(original, sealed),
        },
        factory,
      );
      continue;
    }
    if (!record.attempt) return record;
    if (record.claimed && snapshot?.key === record.attempt.key)
      return { ...record, attempt: snapshot };
    const original = await codec("open", owner, record.attempt.sealed);
    const decoded = {
      ...original,
      kind: record.attempt.kind,
      checks: record.attempt.checks,
      fingerprint: JSON.stringify([original.path, original.body]),
    };
    if (original.key !== record.attempt.key || !originalValid(decoded))
      throw unreadable();
    return { ...record, attempt: decoded };
  }
  throw new Error(
    "Save recovery changed in another tab. Retry recovery storage.",
  );
}
function open(factory) {
  return new Promise((resolve, reject) => {
    let request,
      expired = false;
    const timer = setTimeout(() => {
      expired = true;
      reject(unavailable());
    }, 5000);
    try {
      request = factory.open(JOURNAL_DATABASE, 1);
      request.onupgradeneeded = () => request.result.createObjectStore(STORE);
      request.onerror = () => {
        clearTimeout(timer);
        reject(unavailable());
      };
      request.onblocked = () => {
        expired = true;
        clearTimeout(timer);
        reject(unavailable());
      };
      request.onsuccess = () => {
        clearTimeout(timer);
        if (expired) request.result.close();
        else resolve(request.result);
      };
    } catch {
      clearTimeout(timer);
      reject(unavailable());
    }
  });
}
async function journalTransaction(
  owner,
  operation,
  options = {},
  factory = globalThis.indexedDB,
) {
  const db = await open(factory);
  let changed = false;
  try {
    const value = await new Promise((resolve, reject) => {
      let tx, result, problem;
      try {
        tx = db.transaction(STORE, "readwrite", { durability: "strict" });
        const deadline = setTimeout(() => {
          problem = unavailable();
          try {
            tx.abort();
          } catch {
            reject(problem);
          }
        }, 5000);
        const store = tx.objectStore(STORE),
          request = store.get(owner);
        request.onsuccess = () => {
          try {
            let record = request.result;
            if (record === undefined) {
              record = {
                version: 2,
                generation: crypto.randomUUID(),
                attempt: null,
              };
              store.put(record, owner);
            }
            // Sign-out can remove malformed payloads without interpreting them.
            if (operation === "forget") {
              if (record.generation === options.generation) {
                record = {
                  version: 2,
                  generation: crypto.randomUUID(),
                  attempt: null,
                };
                store.put(record, owner);
                changed = true;
              }
              result = record;
              return;
            }
            if (record.version === 1 && record.attempt !== null) {
              if (
                !originalValid(record.attempt) ||
                Object.keys(record).sort().join() !==
                  "attempt,generation,version" ||
                !/^[a-f0-9-]{36}$/.test(record.generation)
              )
                throw unreadable();
              if (
                operation === "upgrade" &&
                JSON.stringify(record) === options.expected
              ) {
                record = {
                  version: 2,
                  generation: record.generation,
                  attempt: options.attempt,
                };
                validate(record);
                store.put(record, owner);
                changed = true;
                result = record;
              } else
                result = {
                  ...record,
                  legacy: true,
                  expected: JSON.stringify(record),
                };
              return;
            }
            if (record.version === 1 && record.attempt === null) {
              record = { ...record, version: 2 };
              store.put(record, owner);
            }
            if (operation === "upgrade") {
              result = record;
              return;
            }
            validate(record);
            if (
              operation !== "read" &&
              record.generation !== options.generation
            )
              throw new Error("This workspace session has changed.");
            if (operation === "claim") {
              if (!record.attempt) {
                validate({ ...record, attempt: options.attempt });
                record.attempt = options.attempt;
                store.put(record, owner);
                changed = true;
              }
              result = { ...record, claimed: changed };
            } else if (operation === "check") {
              if (
                record.attempt?.key === options.key &&
                record.attempt.kind === "unknown"
              ) {
                record.attempt.checks++;
                if (!Number.isSafeInteger(record.attempt.checks))
                  throw new Error(
                    "The original save needs support before another check.",
                  );
                store.put(record, owner);
                changed = true;
              }
              result = record;
            } else if (["confirmed", "stopped", "remove"].includes(operation)) {
              if (
                record.attempt?.key === options.key &&
                (options.expectedChecks === undefined ||
                  record.attempt.checks === options.expectedChecks)
              ) {
                if (operation === "remove") record.attempt = null;
                else if (record.attempt.kind === "unknown")
                  record.attempt.kind = operation;
                store.put(record, owner);
                changed = true;
              }
              result = { ...record, changed };
            } else if (operation === "read") result = record;
            else throw new Error("Invalid recovery storage operation.");
          } catch (error) {
            problem = error.name === "Error" ? error : unavailable();
            tx.abort();
          }
        };
        tx.oncomplete = () => {
          clearTimeout(deadline);
          resolve(result);
        };
        tx.onabort = () => {
          clearTimeout(deadline);
          reject(problem || unavailable());
        };
        tx.onerror = () => {}; // abort reports the final transaction outcome
      } catch {
        reject(unavailable());
      }
    });
    if (changed && typeof BroadcastChannel !== "undefined") {
      try {
        const channel = new BroadcastChannel(JOURNAL_CHANNEL);
        channel.postMessage({ owner });
        channel.close();
      } catch {
        /* Notifications are optional; transactions are authoritative. */
      }
    }
    return value;
  } finally {
    db.close();
  }
}

// SDK sign-out/session replacement can happen outside the workspace's own
// button. Remove that authenticated session's retained inputs across its
// organizations; another account/session's records remain untouched.
export async function forgetClerkJournalSession(session) {
  const db = await open(globalThis.indexedDB),
    owners = [];
  try {
    await new Promise((resolve, reject) => {
      const tx = db.transaction(STORE, "readwrite", { durability: "strict" });
      const deadline = setTimeout(() => {
        try {
          tx.abort();
        } catch {
          reject(unavailable());
        }
      }, 5000);
      const cursor = tx.objectStore(STORE).openCursor();
      cursor.onsuccess = () => {
        const entry = cursor.result;
        if (!entry) return;
        let scope;
        try {
          scope = JSON.parse(entry.key);
        } catch {
          /* Retain unknown owners. */
        }
        if (
          Array.isArray(scope) &&
          scope[0] === "clerk" &&
          scope[4] === session
        ) {
          try {
            entry.update({
              version: 2,
              generation: crypto.randomUUID(),
              attempt: null,
            });
            owners.push(entry.key);
          } catch {
            tx.abort();
            return;
          }
        }
        entry.continue();
      };
      tx.oncomplete = () => {
        clearTimeout(deadline);
        resolve();
      };
      tx.onabort = () => {
        clearTimeout(deadline);
        reject(unavailable());
      };
    });
  } finally {
    db.close();
  }
  if (typeof BroadcastChannel !== "undefined") {
    try {
      const channel = new BroadcastChannel(JOURNAL_CHANNEL);
      for (const owner of owners) channel.postMessage({ owner });
      channel.close();
    } catch {
      /* Optional invalidation hint. */
    }
  }
}
