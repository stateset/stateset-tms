export function originalValid(attempt) {
  if (
    !attempt ||
    !["unknown", "confirmed", "stopped"].includes(attempt.kind) ||
    typeof attempt.key !== "string" ||
    !/^[\x20-\x7e]{1,200}$/.test(attempt.key) ||
    typeof attempt.message !== "string" ||
    attempt.message.length > 1000 ||
    typeof attempt.createdAt !== "string" ||
    attempt.createdAt.length > 64 ||
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
