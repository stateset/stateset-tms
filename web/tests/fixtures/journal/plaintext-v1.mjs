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
function originalValid(attempt) {
  if (
    !attempt ||
    !["unknown", "confirmed", "stopped"].includes(attempt.kind) ||
    typeof attempt.key !== "string" ||
    !/^[\x20-\x7e]{1,200}$/.test(attempt.key) ||
    typeof attempt.message !== "string" ||
    attempt.message.length > 1000 ||
    typeof attempt.createdAt !== "string" ||
    !Number.isFinite(Date.parse(attempt.createdAt)) ||
    typeof attempt.path !== "string" ||
    !Number.isSafeInteger(attempt.checks) ||
    attempt.checks < 0
  )
    return false;
  const p = attempt.path.split("/");
  if (p.some((part) => !/^[A-Za-z0-9_-]{1,200}$/.test(part))) return false;
  const allowed =
    (p.length === 1 &&
      [
        "loads",
        "invoices",
        "customers",
        "carriers",
        "drivers",
        "equipment",
      ].includes(p[0])) ||
    (p.length === 2 &&
      ["customers", "carriers", "drivers", "equipment"].includes(p[0])) ||
    attempt.path === "sync/import" ||
    (p.length === 3 &&
      ((p[0] === "loads" &&
        ["plan", "commands", "settlement"].includes(p[2])) ||
        (p[0] === "invoices" && ["resolve", "approve"].includes(p[2])) ||
        (["inbox", "outbox"].includes(p[0]) && p[2] === "retry"))) ||
    (p.length === 4 && p[0] === "loads" && p[2] === "pod" && p[3] === "review");
  if (
    !allowed ||
    !attempt.body ||
    typeof attempt.body !== "object" ||
    Array.isArray(attempt.body)
  )
    return false;
  try {
    return (
      new TextEncoder().encode(JSON.stringify(attempt.body)).length <=
        1024 * 1024 &&
      attempt.fingerprint === JSON.stringify([attempt.path, attempt.body])
    );
  } catch {
    return false;
  }
}
function validate(record) {
  if (
    !record ||
    record.version !== 1 ||
    typeof record.generation !== "string" ||
    !/^[a-f0-9-]{36}$/.test(record.generation) ||
    (record.attempt !== null && !originalValid(record.attempt))
  )
    throw new Error(
      "The saved recovery record cannot be read safely. Shared-workspace saves are paused; retain this browser's data for support.",
    );
  return record;
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
export async function saveJournal(
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
                version: 1,
                generation: crypto.randomUUID(),
                attempt: null,
              };
              store.put(record, owner);
            }
            // Sign-out can remove malformed payloads without interpreting them.
            if (operation === "forget") {
              if (record.generation === options.generation) {
                record = {
                  version: 1,
                  generation: crypto.randomUUID(),
                  attempt: null,
                };
                store.put(record, owner);
                changed = true;
              }
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
                if (!originalValid(options.attempt))
                  throw new Error(
                    "The original save cannot be retained safely.",
                  );
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
              version: 1,
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
