import {
  createCipheriv,
  createDecipheriv,
  createHash,
  randomBytes,
} from "node:crypto";

const name = "freight_session";
function key() {
  const secret = process.env.FREIGHT_SESSION_SECRET || "";
  if (secret.length < 32)
    throw new Error("FREIGHT_SESSION_SECRET must have at least 32 characters");
  return createHash("sha256").update(secret).digest();
}
export function seal(token) {
  const iv = randomBytes(12);
  const cipher = createCipheriv("aes-256-gcm", key(), iv);
  const encrypted = Buffer.concat([
    cipher.update(JSON.stringify({ token, expires: Date.now() + 8 * 3600000 })),
    cipher.final(),
  ]);
  return Buffer.concat([iv, cipher.getAuthTag(), encrypted]).toString(
    "base64url",
  );
}
export function tokenFor(req) {
  if (process.env.FREIGHT_DEMO === "true") return "freight-local-demo";
  try {
    const cookie = (req.headers.cookie || "")
      .split(";")
      .map((v) => v.trim())
      .find((v) => v.startsWith(`${name}=`));
    if (!cookie) return null;
    const bytes = Buffer.from(cookie.slice(name.length + 1), "base64url");
    const decipher = createDecipheriv(
      "aes-256-gcm",
      key(),
      bytes.subarray(0, 12),
    );
    decipher.setAuthTag(bytes.subarray(12, 28));
    const session = JSON.parse(
      Buffer.concat([
        decipher.update(bytes.subarray(28)),
        decipher.final(),
      ]).toString(),
    );
    return session.expires > Date.now() ? session.token : null;
  } catch {
    return null;
  }
}
export function cookie(value, remove = false) {
  return `${name}=${value}; Path=/; HttpOnly; SameSite=Strict; Max-Age=${remove ? 0 : 8 * 3600}${process.env.FREIGHT_COOKIE_SECURE === "true" ? "; Secure" : ""}`;
}
export function validOrigin(req) {
  const expected =
    process.env.FREIGHT_WEB_ORIGIN || `http://${req.headers.host}`;
  return req.headers.origin === expected;
}
