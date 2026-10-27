import test from "node:test";
import assert from "node:assert/strict";
import { verifyExpectedAccess } from "../lib/expected-access.mjs";
import { journalOwner, expectedAccess } from "../lib/save-journal.mjs";
const access = { tenant_id: "owned-tenant", access: { subject: "user:Zoë" } };
const owner = journalOwner(access, "credential");
test("recovery scope binds verified tenant, subject, auth mode and organization without credentials", () => {
  assert.notEqual(owner, journalOwner(access, "clerk", "org_a"));
  assert.notEqual(
    journalOwner(access, "clerk", "org_a", "session_a"),
    journalOwner(access, "clerk", "org_a", "session_b"),
  );
  assert.notEqual(
    journalOwner(access, "clerk", "org_a"),
    journalOwner(access, "clerk", "org_b"),
  );
  assert.notEqual(
    owner,
    journalOwner({ ...access, access: { subject: "other" } }, "credential"),
  );
  assert.throws(() =>
    journalOwner({ tenant_id: "only-client-claim" }, "credential"),
  );
  assert.deepEqual(JSON.parse(decodeURIComponent(expectedAccess(owner))), {
    tenant_id: "owned-tenant",
    subject: "user:Zoë",
  });
});
test("reviewed-access preflight uses the captured identity and checks the returned tenant and subject", async () => {
  const calls = [],
    headers = {
      Authorization: "Bearer owned-test-token",
      "X-Freight-Organization-Id": "org_a",
    };
  const verify = async (response) =>
    verifyExpectedAccess(
      expectedAccess(owner),
      "http://owned.invalid",
      headers,
      async (...args) => {
        calls.push(args);
        return Response.json(response);
      },
    );
  assert.equal(await verify(access), null);
  assert.equal(calls[0][0], "http://owned.invalid/v1/access");
  assert.deepEqual(calls[0][1].headers, headers);
  assert.equal(
    (await verify({ ...access, tenant_id: "other" })).body.error.code,
    "workspace_session_changed",
  );
  assert.equal(
    (await verify({ ...access, access: { subject: "other" } })).status,
    409,
  );
});
test("absent hints preserve existing routes while malformed hints never contact upstream", async () => {
  const fetcher = () => {
    throw new Error("must not fetch");
  };
  assert.equal(await verifyExpectedAccess(undefined, "", {}, fetcher), null);
  for (const value of [
    "%",
    [],
    "x".repeat(2001),
    encodeURIComponent('{"tenant_id":"x","subject":"y","role":"admin"}'),
  ]) {
    assert.equal(
      (await verifyExpectedAccess(value, "", {}, fetcher)).status,
      400,
    );
  }
});
test("access denial or unavailable authority cannot authorize a hinted save", async () => {
  for (const status of [401, 403, 503]) {
    const failure = await verifyExpectedAccess(
      expectedAccess(owner),
      "http://owned.invalid",
      {},
      async () =>
        Response.json(
          { error: { message: "Owned authority refusal" } },
          { status, headers: { "Retry-After": "2" } },
        ),
    );
    assert.equal(failure.status, status);
    assert.equal(failure.retryAfter, "2");
    assert.equal(failure.body.error.message, "Owned authority refusal");
  }
});

test("reviewed access refuses actual HTTP redirects before any second endpoint receives the credential", async () => {
  const { createServer } = await import("node:http");
  const { once } = await import("node:events");
  let status = 302,
    redirected = 0;
  const server = createServer((req, res) => {
    if (req.url === "/v1/access") {
      res.writeHead(status, { Location: "/unexpected-access" }).end();
    } else {
      redirected++;
      res.setHeader("Content-Type", "application/json");
      res.end(JSON.stringify(access));
    }
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  try {
    for (status of [301, 302, 303, 307, 308]) {
      await assert.rejects(
        verifyExpectedAccess(
          expectedAccess(owner),
          `http://127.0.0.1:${server.address().port}`,
          { Authorization: "Bearer synthetic-owned-credential" },
        ),
      );
    }
    assert.equal(redirected, 0);
  } finally {
    server.closeAllConnections();
    await new Promise((done) => server.close(done));
  }
});
