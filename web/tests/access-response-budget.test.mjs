import test, { before, after } from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { once } from "node:events";
import { Readable } from "node:stream";
import { readFile, mkdtemp, open } from "node:fs/promises";
import { spawn } from "node:child_process";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { gzipSync } from "node:zlib";
import { seal } from "../lib/session.mjs";

const production = process.env.FREIGHT_ACCESS_TEST_PRODUCTION === "true";
const limit = 64 * 1024;
const token = "owned-access-response-test-credential-at-least-32-characters";
const access = {
  tenant_id: "access-budget",
  access: { subject: "test:operator", capabilities: ["workspace.read"] },
};
const expected = encodeURIComponent(
  JSON.stringify({
    tenant_id: access.tenant_id,
    subject: access.access.subject,
  }),
);
const intent = {
  key: "owned-access-budget-request",
  path: "customers",
  body: { name: "Original customer" },
  message: "Customer added",
  createdAt: new Date().toISOString(),
};
const journal = {
  action: "seal",
  owner: JSON.stringify([
    "credential",
    "",
    access.tenant_id,
    access.access.subject,
    "",
  ]),
  intent,
};
let scenario, calls, origin, api, child, childExit, cookie, handlers;
const settings = {
  FREIGHT_WEB_AUTH_MODE: "credential",
  FREIGHT_DEMO: "false",
  FREIGHT_COOKIE_SECURE: "false",
  FREIGHT_SESSION_SECRET:
    "owned-access-budget-session-secret-at-least-32-characters",
};
const previous = {};
async function listen(server) {
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  return `http://127.0.0.1:${server.address().port}`;
}
async function close(server) {
  if (!server) return;
  server.closeAllConnections();
  await new Promise((resolve) => server.close(resolve));
}
async function loadRoute(path) {
  let source = await readFile(
    new URL(`../pages/api/${path}.js`, import.meta.url),
    "utf8",
  );
  source = source.replaceAll(
    /(?:\.\.\/)+lib\/([a-z-]+\.mjs)/g,
    (_, name) => new URL(`../lib/${name}`, import.meta.url).href,
  );
  return (
    await import(
      `data:text/javascript;base64,${Buffer.from(source).toString("base64")}`
    )
  ).default;
}
before(async () => {
  api = createServer(async (req, res) => {
    for await (const chunk of req) {
    } // Receive the complete original request.
    if (req.headers.authorization !== `Bearer ${token}`)
      return res.writeHead(401).end();
    calls.push(req.url);
    if (req.url !== "/v1/access" && req.url !== "/v1/portal") {
      return res
        .writeHead(200, { "Content-Type": "application/json" })
        .end('{"ok":true}');
    }
    const current = scenario;
    res.on("close", () => {
      current.closed = true;
    });
    let body = current.body ?? Buffer.from(JSON.stringify(access));
    const headers = { "Content-Type": "application/json", "Retry-After": "2" };
    if (current.framing === "gzip") {
      body = gzipSync(body);
      headers["Content-Encoding"] = "gzip";
    }
    if (current.framing !== "chunked" && !current.held)
      headers["Content-Length"] = body.length;
    if (current.framing === "declared") headers["Content-Length"] = limit + 1;
    res.writeHead(current.status ?? 200, headers);
    res.flushHeaders();
    if (current.held) res.write(body);
    else if (current.framing === "chunked") {
      res.write(body.subarray(0, 100));
      res.end(body.subarray(100));
    } else res.end(body);
  });
  const apiUrl = await listen(api);
  const reservation = createServer();
  origin = await listen(reservation);
  const port = reservation.address().port;
  await close(reservation);
  Object.assign(settings, {
    FREIGHT_API_URL: apiUrl,
    FREIGHT_WEB_ORIGIN: origin,
  });
  for (const [key, value] of Object.entries(settings)) {
    previous[key] = process.env[key];
    process.env[key] = value;
  }
  cookie = `freight_session=${seal(token)}`;
  if (production) {
    const dir = await mkdtemp(join(tmpdir(), "freight-access-budget-"));
    const log = await open(join(dir, "web.log"), "wx");
    const root = new URL("../.next/standalone/", import.meta.url);
    child = spawn(process.execPath, [new URL("server.js", root).pathname], {
      cwd: root,
      env: {
        PATH: process.env.PATH,
        NODE_ENV: "production",
        ...settings,
        HOSTNAME: "127.0.0.1",
        PORT: String(port),
      },
      stdio: ["ignore", log.fd, log.fd],
    });
    childExit = once(child, "exit");
    await log.close();
    console.log(`Owned production server log: ${join(dir, "web.log")}`);
    let ready = false;
    for (let i = 0; i < 100; i++) {
      if (child.exitCode !== null) throw new Error("Owned web server exited");
      try {
        const res = await fetch(`${origin}/api/session`, {
          signal: AbortSignal.timeout(500),
        });
        await res.arrayBuffer();
        if (res.status === 405) {
          ready = true;
          break;
        }
      } catch {}
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    assert(ready, "Owned production server readiness");
  } else {
    handlers = Object.fromEntries(
      await Promise.all(
        [
          "session",
          "portal-session",
          "freight/[...path]",
          "freight/save-journal",
        ].map(async (path) => [path, await loadRoute(path)]),
      ),
    );
  }
});
after(async () => {
  if (child && child.exitCode === null) {
    child.kill("SIGKILL");
    await childExit;
  }
  await close(api);
  for (const [key, value] of Object.entries(previous)) {
    if (value === undefined) delete process.env[key];
    else process.env[key] = value;
  }
});
const entries = [
  { name: "staff sign-in", route: "session", body: { token } },
  { name: "partner sign-in", route: "portal-session", body: { token } },
  {
    name: "reviewed save",
    route: "freight/[...path]",
    path: "freight/customers",
    body: intent.body,
    headers: {
      "x-freight-expected-access": expected,
      "idempotency-key": intent.key,
    },
  },
  {
    name: "recovery authorization",
    route: "freight/save-journal",
    body: journal,
  },
];
async function request(entry) {
  const headers = {
    origin,
    cookie,
    "content-type": "application/json",
    ...entry.headers,
  };
  if (production) {
    const res = await fetch(`${origin}/api/${entry.path || entry.route}`, {
      method: "POST",
      headers,
      body: JSON.stringify(entry.body),
      redirect: "error",
      signal: AbortSignal.timeout(15000),
    });
    return {
      status: res.status,
      headers: Object.fromEntries(res.headers),
      body: await res.json(),
    };
  }
  const req = Readable.from([Buffer.from(JSON.stringify(entry.body))]);
  Object.assign(req, {
    method: "POST",
    headers,
    query: { path: ["customers"] },
    body: entry.body,
  });
  const res = {
    statusCode: 200,
    headers: {},
    setHeader(k, v) {
      this.headers[k.toLowerCase()] = v;
      return this;
    },
    status(s) {
      this.statusCode = s;
      return this;
    },
    json(v) {
      this.body = v;
      return this;
    },
    send(v) {
      this.body = JSON.parse(v.toString());
      return this;
    },
    end() {
      return this;
    },
  };
  await handlers[entry.route](req, res);
  return { status: res.statusCode, headers: res.headers, body: res.body };
}
const padded = (bytes) =>
  Buffer.from(JSON.stringify(access).padEnd(bytes, " "));
for (const entry of entries) {
  for (const framing of ["fixed", "chunked", "gzip"]) {
    test(`${entry.name}: refuses oversized ${framing} authority response without mutation or new cookie`, async () => {
      scenario = {
        status: entry.route.includes("session") ? 503 : 200,
        framing,
        body: padded(limit + 1),
      };
      calls = [];
      const res = await request(entry);
      assert.equal(res.status, 503);
      assert.equal(res.headers["cache-control"], "no-store");
      assert.equal(res.headers["set-cookie"], undefined);
      assert.equal(
        calls.length,
        1,
        "No automatic retry or mutation after failed verification",
      );
      assert(!JSON.stringify(res.body).includes(token));
      // Bounded failures are transport failures, never preserved remote busy codes.
      assert.equal(res.body.error?.code, undefined);
      // This marker distinguishes bounded refusal from the old sign-in error parser.
      if (entry.route.includes("session"))
        assert.match(res.body.error.message, /Sign-in unavailable/);
    });
  }
  test(`${entry.name}: ordinary denial and busy response preserve status and retry hint`, async () => {
    for (const status of [401, 403, 503]) {
      scenario = {
        status,
        body: Buffer.from(
          JSON.stringify({
            error: {
              code: "read_capacity_exceeded",
              message: "Owned authority refusal",
            },
          }),
        ),
      };
      calls = [];
      const res = await request(entry);
      assert.equal(res.status, status);
      assert.equal(res.headers["set-cookie"], undefined);
      assert.equal(calls.length, 1);
      if (status === 503) {
        assert.equal(res.headers["retry-after"], "2");
        assert.equal(res.body.error.code, "read_capacity_exceeded");
      }
    }
  });
  test(`${entry.name}: exact-limit valid identity remains usable`, async () => {
    scenario = { body: padded(limit) };
    calls = [];
    const res = await request(entry);
    assert.equal(res.status, 200);
    assert.equal(calls.length, entry.path ? 2 : 1);
    if (entry.route.includes("session"))
      assert(res.headers["set-cookie"]?.includes("HttpOnly"));
  });
}
for (const entry of entries.slice(2)) {
  test(`${entry.name}: an oversized denial is unavailable rather than a session revocation`, async () => {
    scenario = { status: 401, framing: "chunked", body: padded(limit + 1) };
    calls = [];
    const res = await request(entry);
    assert.equal(res.status, 503);
    assert.equal(calls.length, 1);
    assert.equal(res.headers["set-cookie"], undefined);
  });
  test(`${entry.name}: malformed success cannot authorize work`, async () => {
    scenario = { body: Buffer.from("{invalid") };
    calls = [];
    const res = await request(entry);
    assert.equal(res.status, 503);
    assert.equal(calls.length, 1);
  });
}
for (const entry of entries.slice(0, 2)) {
  test(`${entry.name}: cancels an unused successful body before it finishes`, async () => {
    scenario = { held: true, body: Buffer.from("{") };
    calls = [];
    const res = await request(entry);
    assert.equal(res.status, 200);
    for (let i = 0; i < 50 && !scenario.closed; i++)
      await new Promise((resolve) => setTimeout(resolve, 10));
    assert(
      scenario.closed,
      "Unused response must be cancelled, not left waiting for the body timeout",
    );
    assert.equal(calls.length, 1);
  });
}

for (const entry of entries.slice(2)) {
  for (const framing of ["declared", "chunked"]) {
    test(`${entry.name}: cancels unfinished ${framing} excess before the API timeout`, async () => {
      scenario = {
        held: true,
        framing,
        body: framing === "declared" ? Buffer.from("{") : padded(limit + 1),
      };
      calls = [];
      const start = performance.now();
      const res = await request(entry);
      assert.equal(res.status, 503);
      assert(
        performance.now() - start < 2000,
        "Size refusal must not wait for the ten-second API timeout",
      );
      for (let i = 0; i < 50 && !scenario.closed; i++)
        await new Promise((resolve) => setTimeout(resolve, 10));
      assert(
        scenario.closed,
        "Oversized unfinished response must be cancelled",
      );
      assert.equal(calls.length, 1);
    });
  }
  test(`${entry.name}: rejects invalid UTF-8 instead of accepting a repaired identity`, async () => {
    scenario = {
      body: Buffer.concat([
        Buffer.from(JSON.stringify(access).slice(0, -1) + ',"unused":"'),
        Buffer.from([0xff]),
        Buffer.from('"}'),
      ]),
    };
    calls = [];
    const res = await request(entry);
    assert.equal(res.status, 503);
    assert.equal(calls.length, 1);
  });
}
for (const entry of entries.slice(0, 2)) {
  test(`${entry.name}: a denied credential cancels its unused stalled body`, async () => {
    scenario = { status: 401, held: true, body: Buffer.from("{") };
    calls = [];
    const res = await request(entry);
    assert.equal(res.status, 401);
    assert.equal(res.headers["set-cookie"], undefined);
    for (let i = 0; i < 50 && !scenario.closed; i++)
      await new Promise((resolve) => setTimeout(resolve, 10));
    assert(
      scenario.closed,
      "Denial body must be cancelled without waiting for its timeout",
    );
    assert.equal(calls.length, 1);
  });
}
