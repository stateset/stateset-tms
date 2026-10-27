import test from "node:test";
import assert from "node:assert/strict";
import { publicAuthConfig, clerkGatewayIdentity } from "../lib/auth-config.mjs";
import {
  organizationFetch,
  organizationHref,
} from "../lib/identity-client.mjs";
const publicKey = `pk_test_${Buffer.from("clerk.fixture.test$").toString("base64url")}`;
const env = {
  FREIGHT_WEB_AUTH_MODE: "clerk",
  FREIGHT_CLERK_PUBLISHABLE_KEY: publicKey,
  FREIGHT_WEB_ORIGIN: "https://freight.example.test",
};

test("staff browser configuration exposes only a public key and rejects insecure or ambiguous modes", () => {
  assert.deepEqual(publicAuthConfig({}), { mode: "credential" });
  assert.deepEqual(
    publicAuthConfig({ ...env, FREIGHT_CLERK_JSON: "must never be returned" }),
    { mode: "clerk", publishableKey: publicKey },
  );
  for (const change of [
    { FREIGHT_WEB_AUTH_MODE: "typo" },
    { FREIGHT_CLERK_PUBLISHABLE_KEY: "sk_live_not_a_public_key" },
    { FREIGHT_WEB_ORIGIN: "http://external.example" },
    { FREIGHT_WEB_ORIGIN: "https://freight.example.test/path" },
    { FREIGHT_DEMO: "true" },
  ])
    assert.throws(() => publicAuthConfig({ ...env, ...change }));
});
test("staff gateway ignores workspace cookies and requires one explicit organization and a short-lived JWT format", () => {
  const request = {
    headers: {
      cookie:
        "freight_session=old_admin_cookie; __session=fixture.payload.signature",
    },
    query: { organization_id: "org_alpha" },
  };
  assert.deepEqual(clerkGatewayIdentity(request), {
    token: "fixture.payload.signature",
    organization: "org_alpha",
  });
  assert.equal(
    clerkGatewayIdentity({
      ...request,
      headers: { cookie: "freight_session=old_admin_cookie" },
    }),
    null,
  );
  assert.equal(clerkGatewayIdentity({ ...request, query: {} }), null);
  assert.equal(
    clerkGatewayIdentity({
      ...request,
      query: { organization_id: ["org_alpha", "org_beta"] },
    }),
    null,
  );
  assert.equal(
    clerkGatewayIdentity({
      ...request,
      headers: { ...request.headers, "x-freight-organization-id": "org_beta" },
    }),
    null,
  );
  assert.equal(
    clerkGatewayIdentity({
      ...request,
      headers: { ...request.headers, authorization: "Basic other_credentials" },
    }),
    null,
  );
  assert.equal(
    clerkGatewayIdentity({
      ...request,
      headers: {
        ...request.headers,
        authorization: "Bearer static_api_credential",
      },
    }),
    null,
  );
  assert.equal(
    clerkGatewayIdentity({
      ...request,
      headers: {
        cookie: "__session=one.two.three; __session=other.value.signature",
      },
    }),
    null,
  );
});
test("staff requests retain the organization that rendered the form and overwrite caller authorization hints", async () => {
  let active = "org_alpha",
    received;
  const bound = organizationFetch(
    "org_alpha",
    async (options) => {
      assert.equal(options.organizationId, "org_alpha");
      return `${active}.payload.signature`;
    },
    async (path, options) => {
      received = { path, options };
      return new Response("{}", { status: 200 });
    },
  );
  active = "org_beta";
  await bound("/api/freight/customers", {
    method: "POST",
    headers: {
      Authorization: "Bearer forged",
      "X-Freight-Organization-Id": "org_beta",
      "Idempotency-Key": "same-retry-key",
    },
    body: '{"name":"Original organization action"}',
  });
  assert.equal(
    received.options.headers.get("authorization"),
    "Bearer org_beta.payload.signature",
  );
  assert.equal(
    received.options.headers.get("x-freight-organization-id"),
    "org_alpha",
    "the server must reject a newly switched token instead of posting an old form into its new organization",
  );
  assert.equal(
    received.options.headers.get("idempotency-key"),
    "same-retry-key",
  );
  assert.equal(
    received.options.body,
    '{"name":"Original organization action"}',
  );
  const absent = organizationFetch(
    "org_alpha",
    async () => null,
    async () => {
      throw new Error("must not forward unauthenticated requests");
    },
  );
  assert.equal((await absent("/api/freight/workspace")).status, 401);
});
test("staff downloads bind the originating organization without placing tokens in URLs", () => {
  const path = "/api/freight/documents/doc_15/content";
  assert.equal(
    organizationHref(path, "org_alpha"),
    `${path}?organization_id=org_alpha`,
  );
  assert.equal(organizationHref(path, null), path);
  assert.equal(
    organizationHref("/api/freight/finance/journal.csv?page=2", "org_beta"),
    "/api/freight/finance/journal.csv?page=2&organization_id=org_beta",
  );
  for (const external of [
    "https://external.example/collect",
    "//external.example/api/freight/documents",
    "/api/freight/../../api/session",
  ])
    assert.throws(() => organizationHref(external, "org_alpha"));
});

for (const name of ["ClerkOfflineError", "AbortError"]) {
  test(`a rejected SDK token (${name}) remains a failure and never becomes a forwarded command or synthetic sign-out`, async () => {
    const failure = Object.assign(new Error("Owned token retrieval failure"), {
      name,
    });
    let forwarded = 0;
    const bound = organizationFetch(
      "org_alpha",
      async () => {
        throw failure;
      },
      async () => {
        forwarded++;
        return new Response("{}");
      },
    );
    await assert.rejects(
      bound("/api/freight/customers", {
        method: "POST",
        headers: { "Idempotency-Key": "retain-original" },
        body: '{"name":"Original unsent command"}',
      }),
      (error) => error === failure,
    );
    assert.equal(forwarded, 0);
  });
}
