import serviceOrigins from "./service-origins.cjs";
const { fixturePort, fixtureOrigin, clerkOrigin } = serviceOrigins();
// Owned Clerk protocol fixture, not an identity provider or real staff account.
import { readFile } from "node:fs/promises";
import { createPrivateKey, sign } from "node:crypto";
const key = createPrivateKey({
  key: await readFile(
    new URL("../../tests/fixtures/identity/key-a.der", import.meta.url),
  ),
  format: "der",
  type: "pkcs1",
});
const jwk = JSON.parse(
  await readFile(
    new URL("../../tests/fixtures/identity/key-a.json", import.meta.url),
    "utf8",
  ),
);
const secret = "Bearer sk_test_owned_fixture_only_at_least_32_characters";
let state = {
  status: "active",
  role: "org:operations",
  member: true,
  org: "org_staff",
  outage: false,
};
async function body(request) {
  const chunks = [];
  for await (const chunk of request) chunks.push(chunk);
  return JSON.parse(Buffer.concat(chunks).toString() || "{}");
}
export async function clerkFixture(request, response) {
  const url = new URL(request.url, fixtureOrigin),
    path = url.pathname;
  if (path === "/.well-known/jwks.json") {
    response.end(JSON.stringify({ keys: [jwk] }));
    return true;
  }
  if (
    path.startsWith("/__identity/") &&
    request.headers["x-fixture-key"] === "freight-fixture-only"
  ) {
    const input = await body(request);
    if (path === "/__identity/control") {
      state = { ...state, ...input };
      response.end('{"ok":true}');
      return true;
    }
    if (path === "/__identity/token") {
      const now = Math.floor(Date.now() / 1000);
      const header = Buffer.from(
        JSON.stringify({ alg: "RS256", kid: "fixture-a", typ: "JWT" }),
      ).toString("base64url");
      const claims = {
        iss: fixtureOrigin,
        sub: "user_staff",
        sid: "sess_staff",
        azp: clerkOrigin,
        iat: now,
        nbf: now - 5,
        exp: now + 60,
        v: 2,
        o: { id: state.org, rol: "admin" },
        ...input,
      };
      const unsigned = `${header}.${Buffer.from(JSON.stringify(claims)).toString("base64url")}`;
      response.end(
        JSON.stringify({
          token: `${unsigned}.${sign("RSA-SHA256", Buffer.from(unsigned), key).toString("base64url")}`,
        }),
      );
      return true;
    }
  }
  if (
    request.headers.authorization === secret &&
    (path.startsWith("/sessions/") || path.startsWith("/organizations/"))
  ) {
    if (state.outage) {
      response.statusCode = 503;
      response.end('{"error":"owned identity outage"}');
      return true;
    }
    const ownedSession =
      /^\/sessions\/(sess_staff|sess_sdk_current|sess_sdk_next)$/.exec(
        path,
      )?.[1];
    if (ownedSession) {
      response.end(
        JSON.stringify({
          id: ownedSession,
          user_id: "user_staff",
          status: state.status,
          actor: null,
          last_active_organization_id: state.org,
          expire_at: Date.now() + 3600000,
        }),
      );
      return true;
    }
    const membershipOrg =
      /^\/organizations\/(org_staff|org_other|org_carrier|org_customer|org_sdk_staff|org_sdk_other|org_sdk_carrier)\/memberships$/.exec(
        path,
      )?.[1];
    if (membershipOrg && url.searchParams.get("user_id") === "user_staff") {
      const data = state.member
        ? [
            {
              organization: { id: membershipOrg },
              public_user_data: { user_id: "user_staff" },
              role: state.role,
            },
          ]
        : [];
      response.end(JSON.stringify({ data, total_count: data.length }));
      return true;
    }
    response.statusCode = 404;
    response.end('{"error":"No such synthetic identity"}');
    return true;
  }
  return false;
}
