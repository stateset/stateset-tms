import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { seal } from "../lib/session.mjs";

test("staff sign-in verifies identity without fetching or decoding workspace history", async (t) => {
  const origin = "http://127.0.0.1:3015";
  for (const [key, value] of Object.entries({
    FREIGHT_WEB_AUTH_MODE: "credential",
    FREIGHT_DEMO: "false",
    FREIGHT_COOKIE_SECURE: "false",
    FREIGHT_WEB_ORIGIN: origin,
    FREIGHT_SESSION_SECRET:
      "owned-catalog-signin-secret-at-least-32-characters",
  })) {
    const previous = process.env[key];
    process.env[key] = value;
    t.after(() => {
      if (previous === undefined) delete process.env[key];
      else process.env[key] = previous;
    });
  }
  let source = await readFile(
    new URL("../pages/api/session.js", import.meta.url),
    "utf8",
  );
  for (const name of ["session", "auth-config", "access-response"])
    source = source.replace(
      `../../lib/${name}.mjs`,
      new URL(`../lib/${name}.mjs`, import.meta.url).href,
    );
  const { default: handler } = await import(
    `data:text/javascript;base64,${Buffer.from(source).toString("base64")}`
  );
  const calls = [];
  t.mock.method(globalThis, "fetch", async (url) => {
    calls.push(new URL(url).pathname);
    return new Response(
      JSON.stringify({ tenant_id: "owned", access: { role: "viewer" } }),
      { status: 200 },
    );
  });
  const response = {
    headers: {},
    setHeader(key, value) {
      this.headers[key.toLowerCase()] = value;
      return this;
    },
    status(code) {
      this.statusCode = code;
      return this;
    },
    json(body) {
      this.body = body;
      return this;
    },
  };
  await handler(
    {
      method: "POST",
      headers: { origin },
      body: { token: "owned-catalog-credential-at-least-32-characters" },
    },
    response,
  );
  assert.deepEqual(calls, ["/v1/access"]);
  assert.equal(response.body.ok, true);
  assert.ok(response.headers["set-cookie"]);
});

test("catalog gateway forwards each permitted filter once and refuses ambiguous repeated filters", async (t) => {
  for (const [key, value] of Object.entries({
    FREIGHT_WEB_AUTH_MODE: "credential",
    FREIGHT_DEMO: "false",
    FREIGHT_SESSION_SECRET:
      "owned-catalog-gateway-secret-at-least-32-characters",
  })) {
    const previous = process.env[key];
    process.env[key] = value;
    t.after(() => {
      if (previous === undefined) delete process.env[key];
      else process.env[key] = previous;
    });
  }
  let source = await readFile(
    new URL("../pages/api/freight/[...path].js", import.meta.url),
    "utf8",
  );
  for (const name of ["session", "gateway", "auth-config", "expected-access"])
    source = source.replace(
      `../../../lib/${name}.mjs`,
      new URL(`../lib/${name}.mjs`, import.meta.url).href,
    );
  const { default: handler } = await import(
    `data:text/javascript;base64,${Buffer.from(source).toString("base64")}`
  );
  const upstream = [];
  t.mock.method(globalThis, "fetch", async (url) => {
    upstream.push(new URL(url));
    return new Response(
      JSON.stringify({ loads: [], next_cursor: null, limit: 25 }),
      { status: 200 },
    );
  });
  const response = () => ({
    headers: {},
    setHeader(key, value) {
      this.headers[key.toLowerCase()] = value;
      return this;
    },
    status(code) {
      this.statusCode = code;
      return this;
    },
    send(body) {
      this.body = body;
      return this;
    },
    json(body) {
      this.body = body;
      return this;
    },
  });
  const filters = {
      limit: "25",
      cursor: "0011",
      status: "planned",
      customer_id: "c1",
      carrier_id: "ca4",
      q: "100%_ Portland",
    },
    headers = {
      cookie: `freight_session=${seal("owned-catalog-credential-at-least-32-characters")}`,
    };
  let res = response();
  await handler(
    {
      method: "GET",
      headers,
      query: { path: ["loads"], ...filters, tenant_id: "forged" },
    },
    res,
  );
  assert.equal(res.statusCode, 200);
  assert.equal(upstream.length, 1);
  for (const [key, value] of Object.entries(filters))
    assert.equal(upstream[0].searchParams.get(key), value);
  assert.equal(upstream[0].searchParams.has("tenant_id"), false);
  for (const key of Object.keys(filters)) {
    res = response();
    await handler(
      {
        method: "GET",
        headers,
        query: { path: ["loads"], ...filters, [key]: ["one", "two"] },
      },
      res,
    );
    assert.equal(res.statusCode, 400);
  }
  assert.equal(upstream.length, 1, "ambiguous filters must not reach upstream");
  const shipment = "00000000-0000-4000-8000-000000000005";
  res = response();
  await handler(
    {
      method: "GET",
      headers,
      query: {
        path: ["loads", "load_native", "commerce-tracking"],
        shipment_id: shipment,
        tenant_id: "forged",
      },
    },
    res,
  );
  assert.equal(res.statusCode, 200);
  assert.equal(upstream[1].searchParams.get("shipment_id"), shipment);
  assert.equal(upstream[1].searchParams.has("tenant_id"), false);
  res = response();
  await handler(
    {
      method: "GET",
      headers,
      query: {
        path: ["loads", "load_native", "commerce-tracking"],
        shipment_id: [shipment, shipment],
      },
    },
    res,
  );
  assert.equal(res.statusCode, 400);
  assert.equal(
    upstream.length,
    2,
    "ambiguous shipment identity must not reach upstream",
  );
  res = response();
  await handler(
    {
      method: "GET",
      headers,
      query: {
        path: ["loads", "load_native", "commerce-shipments"],
        page: "2",
      },
    },
    res,
  );
  assert.equal(res.statusCode, 200);
  assert.equal(
    upstream[2].pathname,
    "/v1/loads/load_native/commerce-shipments",
  );
  assert.deepEqual([...upstream[2].searchParams], [["page", "2"]]);
  for (const forged of [
    { page: ["1", "2"] },
    { order_id: "another-order" },
    { tenant_id: "another-tenant" },
    { url: "https://forged.test" },
    { offset: "25" },
  ]) {
    res = response();
    await handler(
      {
        method: "GET",
        headers,
        query: {
          path: ["loads", "load_native", "commerce-shipments"],
          ...forged,
        },
      },
      res,
    );
    assert.equal(res.statusCode, 400);
  }
  assert.equal(
    upstream.length,
    3,
    "native discovery overrides must not reach upstream",
  );
});
