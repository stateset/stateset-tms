import test from "node:test";
import assert from "node:assert/strict";
import { cookie, seal, tokenFor, validOrigin } from "../lib/session.mjs";

process.env.FREIGHT_DEMO = "false";
process.env.FREIGHT_SESSION_SECRET =
  "a-test-session-secret-with-at-least-32-characters";
const credential = "tenant-operator-credential-at-least-32-characters";

test("encrypted sessions roundtrip without exposing the credential", () => {
  const value = seal(credential);
  assert.equal(value.includes(credential), false);
  assert.equal(
    tokenFor({ headers: { cookie: `freight_session=${value}` } }),
    credential,
  );
  assert.equal(tokenFor({ headers: {} }), null);
  const tampered = Buffer.from(value, "base64url");
  tampered[30] ^= 1;
  assert.equal(
    tokenFor({
      headers: { cookie: `freight_session=${tampered.toString("base64url")}` },
    }),
    null,
  );
});
test("sessions expire after eight hours", () => {
  const now = Date.now;
  const value = seal(credential);
  try {
    Date.now = () => now() + 9 * 3600000;
    assert.equal(
      tokenFor({ headers: { cookie: `freight_session=${value}` } }),
      null,
    );
  } finally {
    Date.now = now;
  }
});
test("mutation origin checks and secure cookie settings", () => {
  process.env.FREIGHT_WEB_ORIGIN = "https://freight.example.test";
  assert.equal(
    validOrigin({
      headers: {
        origin: "https://attacker.test",
        host: "freight.example.test",
      },
    }),
    false,
  );
  assert.equal(
    validOrigin({ headers: { origin: "https://freight.example.test" } }),
    true,
  );
  process.env.FREIGHT_COOKIE_SECURE = "true";
  assert.match(cookie("value"), /HttpOnly; SameSite=Strict/);
  assert.match(cookie("value"), /; Secure$/);
  assert.match(cookie("", true), /Max-Age=0/);
});
