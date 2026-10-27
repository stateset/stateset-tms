import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { seal } from "../lib/session.mjs";

test("authenticated gateway preserves the API busy code and retry hint without retrying or changing the session", async (t) => {
  const settings = {
    FREIGHT_WEB_AUTH_MODE: "credential",
    FREIGHT_DEMO: "false",
    FREIGHT_SESSION_SECRET:
      "owned-gateway-session-secret-at-least-32-characters",
  };
  for (const [key, value] of Object.entries(settings)) {
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
  const body = {
    error: {
      code: "read_capacity_exceeded",
      message: "Workspace is busy. Please retry shortly.",
    },
  };
  let calls = 0;
  t.mock.method(globalThis, "fetch", async () => {
    calls++;
    return new Response(JSON.stringify(body), {
      status: 503,
      headers: { "retry-after": "2", "content-type": "application/json" },
    });
  });
  const headers = {},
    response = {
      setHeader(key, value) {
        headers[key.toLowerCase()] = value;
        return this;
      },
      status(value) {
        this.statusCode = value;
        return this;
      },
      send(value) {
        this.body = value;
        return this;
      },
    };
  await handler(
    {
      method: "GET",
      headers: {
        cookie: `freight_session=${seal("owned-gateway-credential-at-least-32-characters")}`,
      },
      query: { path: ["workspace"] },
    },
    response,
  );
  assert.equal(response.statusCode, 503);
  assert.equal(headers["retry-after"], "2");
  assert.equal(headers["cache-control"], "no-store");
  assert.equal(headers["set-cookie"], undefined);
  assert.deepEqual(JSON.parse(response.body.toString()), body);
  assert.equal(calls, 1);
});

test("sign-in distinguishes read overload from rejected credentials and issues no session until verification succeeds", async (t) => {
  const origin = "http://127.0.0.1:3015";
  for (const [key, value] of Object.entries({
    FREIGHT_WEB_AUTH_MODE: "credential",
    FREIGHT_DEMO: "false",
    FREIGHT_COOKIE_SECURE: "false",
    FREIGHT_WEB_ORIGIN: origin,
    FREIGHT_SESSION_SECRET: "owned-unit-session-secret-at-least-32-characters",
  })) {
    const previous = process.env[key];
    process.env[key] = value;
    t.after(() => {
      if (previous === undefined) delete process.env[key];
      else process.env[key] = previous;
    });
  }
  for (const entry of ["session", "portal-session"]) {
    let source = await readFile(
      new URL(`../pages/api/${entry}.js`, import.meta.url),
      "utf8",
    );
    for (const name of ["session", "auth-config", "access-response"]) {
      source = source.replace(
        `../../lib/${name}.mjs`,
        new URL(`../lib/${name}.mjs`, import.meta.url).href,
      );
    }
    const { default: handler } = await import(
      `data:text/javascript;base64,${Buffer.from(source).toString("base64")}`
    );
    const credential = "owned-unit-operator-credential-at-least-32-characters";
    for (const [status, body, expected, code] of [
      [
        503,
        {
          error: {
            code: "read_capacity_exceeded",
            message: "Workspace is busy",
          },
        },
        503,
        "read_capacity_exceeded",
      ],
      [
        503,
        {
          error: {
            code: "database_temporarily_unavailable",
            message: "internal detail",
          },
        },
        503,
        "database_temporarily_unavailable",
      ],
      [503, { error: { message: "internal detail" } }, 503, undefined],
      [401, { error: { message: "invalid" } }, 401, undefined],
      [200, { loads: [] }, 200, undefined],
    ]) {
      let calls = 0;
      const mock = t.mock.method(globalThis, "fetch", async () => {
        calls++;
        return new Response(JSON.stringify(body), {
          status,
          headers: { "retry-after": "2" },
        });
      });
      const headers = {};
      const response = {
        statusCode: 200,
        setHeader(k, v) {
          headers[k.toLowerCase()] = v;
          return this;
        },
        status(value) {
          this.statusCode = value;
          return this;
        },
        json(value) {
          this.body = value;
          return this;
        },
      };
      await handler(
        { method: "POST", headers: { origin }, body: { token: credential } },
        response,
      );
      assert.equal(response.statusCode, expected);
      assert.equal(
        calls,
        1,
        "sign-in never automatically retries verification",
      );
      assert.equal(response.body.error?.code, code);
      assert.equal(headers["cache-control"], "no-store");
      assert.equal(Boolean(headers["set-cookie"]), status === 200);
      if (status === 503)
        assert.doesNotMatch(
          response.body.error.message,
          /credential|internal detail/i,
        );
      if (code) assert.equal(headers["retry-after"], "2");
      assert.ok(!JSON.stringify(response.body).includes(credential));
      mock.mock.restore();
    }
  }
});
