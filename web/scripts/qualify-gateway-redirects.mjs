// Qualify the actual production gateway against owned loopback HTTP peers.
// Build first. This never uses a deployment URL, database, or real credential.
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { createHash, randomBytes, randomUUID } from "node:crypto";
import { mkdtemp, readFile, writeFile, open } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import assert from "node:assert/strict";

const output = process.argv[2];
if (!output) throw new Error("Pass a new output JSON path");
const outputFile = await open(output, "wx");
const root = fileURLToPath(new URL("../", import.meta.url));
const artifacts = await mkdtemp(join(tmpdir(), "freight-gateway-redirects-"));
const sha = (value) => createHash("sha256").update(value).digest("hex");
const report = {
  scope:
    "Production Next.js gateway; synthetic loopback API/redirect destinations, no Rust or live identity provider in this fixture",
  result: "running",
  started_at: new Date().toISOString(),
  node: process.version,
  artifacts,
  checks: [],
  harness_sha256: sha(await readFile(fileURLToPath(import.meta.url))),
  build_id: (await readFile(join(root, ".next/BUILD_ID"), "utf8")).trim(),
};
const access = {
  tenant_id: "redirect-fixture",
  access: { subject: "fixture:operator", capabilities: ["workspace.read"] },
};
const token = randomBytes(32).toString("hex");
const sinkPath = `/capture-${randomUUID()}`;
const binary = Buffer.from([0, 255, 128, 10, 13, 65]);
let mode = null,
  sourceCalls = [],
  sinkCalls = [],
  child;
const json = (res, value) => {
  res.setHeader("Content-Type", "application/json");
  res.end(JSON.stringify(value));
};
async function listen(server) {
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  return `http://127.0.0.1:${server.address().port}`;
}
async function close(server) {
  server.closeAllConnections();
  await new Promise((done) => server.close(done));
}
const sink = createServer(async (req, res) => {
  if (req.url !== sinkPath) {
    res.writeHead(404).end();
    return;
  }
  const chunks = [];
  for await (const chunk of req) chunks.push(chunk);
  sinkCalls.push({
    method: req.method,
    bytes: Buffer.concat(chunks).length,
    credentialForwarded: req.headers.authorization === `Bearer ${token}`,
  });
  json(res, access);
});
const api = createServer(async (req, res) => {
  if (req.url === sinkPath) {
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    sinkCalls.push({
      method: req.method,
      bytes: Buffer.concat(chunks).length,
      credentialForwarded: req.headers.authorization === `Bearer ${token}`,
    });
    json(res, access);
    return;
  }
  if (req.headers.authorization !== `Bearer ${token}`) {
    res.writeHead(401).end();
    return;
  }
  const chunks = [];
  for await (const chunk of req) chunks.push(chunk);
  const body = Buffer.concat(chunks),
    path = new URL(req.url, "http://fixture.invalid").pathname;
  sourceCalls.push({
    path,
    method: req.method,
    body,
    key: req.headers["idempotency-key"],
  });
  if (mode?.path === path) {
    res.writeHead(mode.status, { Location: mode.location }).end();
    return;
  }
  if (path === "/v1/access" || path === "/v1/portal") json(res, access);
  else if (path === "/v1/documents/proof/content") {
    res.writeHead(200, {
      "Content-Type": "application/octet-stream",
      "X-Content-Sha256": sha(binary),
    });
    res.end(binary);
  } else json(res, { data: { accepted: true }, bytes: body.length });
});
async function check(name, run) {
  try {
    await run();
    report.checks.push({ name, result: "pass" });
  } catch (error) {
    report.checks.push({ name, result: "failed", error: error.message });
  }
  if (mode)
    report.checks.at(-1).wire = {
      configuredRequests: sourceCalls.length,
      redirectRequests: sinkCalls.length,
      redirectedBodyBytes: sinkCalls.reduce((n, call) => n + call.bytes, 0),
      redirectedCredentials: sinkCalls.filter(
        (call) => call.credentialForwarded,
      ).length,
    };
}
try {
  const sinkUrl = await listen(sink),
    apiUrl = await listen(api);
  const reservation = createServer();
  const webUrl = await listen(reservation);
  const port = reservation.address().port;
  await close(reservation);
  const log = await open(join(artifacts, "web.log"), "wx");
  child = spawn(process.execPath, [join(root, ".next/standalone/server.js")], {
    cwd: join(root, ".next/standalone"),
    env: {
      PATH: process.env.PATH,
      NODE_ENV: "production",
      PORT: String(port),
      HOSTNAME: "127.0.0.1",
      FREIGHT_DEMO: "false",
      FREIGHT_WEB_AUTH_MODE: "credential",
      FREIGHT_API_URL: apiUrl,
      FREIGHT_WEB_ORIGIN: webUrl,
      FREIGHT_SESSION_SECRET: randomBytes(32).toString("hex"),
      FREIGHT_COOKIE_SECURE: "false",
    },
    stdio: ["ignore", log.fd, log.fd],
  });
  await log.close();
  const exit = once(child, "exit");
  let ready = false;
  for (let n = 0; n < 100; n++) {
    if (child.exitCode !== null)
      throw new Error("Owned web process exited before readiness");
    try {
      const r = await fetch(`${webUrl}/api/session`, {
        signal: AbortSignal.timeout(500),
      });
      await r.arrayBuffer();
      if (r.status === 405) {
        ready = true;
        break;
      }
    } catch {}
    await new Promise((done) => setTimeout(done, 100));
  }
  assert(ready, "Owned web readiness deadline");
  let cookie;
  const request = async (path, body, extra = {}) => {
    const r = await fetch(webUrl + path, {
      method: body === undefined ? "GET" : "POST",
      redirect: "error",
      headers: {
        Origin: webUrl,
        ...(cookie ? { Cookie: cookie } : {}),
        "Content-Type": "application/json",
        ...extra,
      },
      ...(body === undefined
        ? {}
        : { body: Buffer.isBuffer(body) ? body : JSON.stringify(body) }),
      signal: AbortSignal.timeout(20000),
    });
    return {
      status: r.status,
      headers: r.headers,
      body: Buffer.from(await r.arrayBuffer()),
    };
  };
  await check(
    "ordinary staff and partner sign-in still establish a session",
    async () => {
      for (const path of ["/api/session", "/api/portal-session"]) {
        const r = await request(path, { token });
        assert.equal(r.status, 200);
        assert(r.headers.get("set-cookie")?.includes("HttpOnly"));
        cookie = r.headers.get("set-cookie").split(";")[0];
      }
    },
  );
  assert(cookie, "The positive control must create a real encrypted cookie");
  const expected = encodeURIComponent(
    JSON.stringify({
      tenant_id: access.tenant_id,
      subject: access.access.subject,
    }),
  );
  const intent = {
    key: randomUUID(),
    path: "customers",
    body: { name: "Synthetic original" },
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
  const cases = [
    {
      name: "staff sign-in",
      path: "/api/session",
      upstream: "/v1/access",
      body: { token },
    },
    {
      name: "partner sign-in",
      path: "/api/portal-session",
      upstream: "/v1/portal",
      body: { token },
    },
    {
      name: "save recovery authorization",
      path: "/api/freight/save-journal",
      upstream: "/v1/access",
      body: journal,
    },
    {
      name: "reviewed-access preflight",
      path: "/api/freight/customers",
      upstream: "/v1/access",
      body: intent.body,
      headers: { "X-Freight-Expected-Access": expected },
    },
    {
      name: "workspace read",
      path: "/api/freight/workspace",
      upstream: "/v1/workspace",
    },
    {
      name: "shipment save",
      path: "/api/freight/loads/load_1/commands",
      upstream: "/v1/loads/load_1/commands",
      body: {
        action: "tracking",
        tracking_number: "Synthetic private shipment",
      },
    },
    {
      name: "document download",
      path: "/api/freight/documents/proof/content",
      upstream: "/v1/documents/proof/content",
    },
    {
      name: "document upload",
      path: "/api/freight/loads/load_1/documents?kind=pod&filename=proof.pdf",
      upstream: "/v1/loads/load_1/documents",
      body: binary,
      headers: { "Content-Type": "application/pdf" },
    },
  ];
  for (const status of [301, 302, 303, 307, 308]) {
    for (const destination of ["same-origin", "cross-origin"]) {
      for (const item of cases) {
        mode = {
          path: item.upstream,
          status,
          location:
            destination === "same-origin" ? sinkPath : sinkUrl + sinkPath,
        };
        sourceCalls = [];
        sinkCalls = [];
        await check(
          `${status} ${destination}: ${item.name} rejects redirect without forwarding or new cookie`,
          async () => {
            const r = await request(item.path, item.body, {
              ...item.headers,
              "Idempotency-Key": intent.key,
            });
            // Check the wire before response status, exposing body leaks in a baseline run.
            assert.equal(
              sinkCalls.length,
              0,
              `Redirect destination received ${sinkCalls.length} request(s), ${sinkCalls.reduce((n, v) => n + v.bytes, 0)} body bytes`,
            );
            assert.equal(
              sourceCalls.length,
              1,
              "No mutation may follow a redirected preflight; no automatic retry",
            );
            assert.equal(r.status, 503);
            assert.equal(r.headers.get("cache-control"), "no-store");
            assert.equal(r.headers.get("set-cookie"), null);
            assert.equal(r.headers.get("location"), null);
            assert(
              !r.body.includes(Buffer.from(token)),
              "Error must not disclose credential",
            );
          },
        );
      }
    }
  }
  mode = null;
  sourceCalls = [];
  sinkCalls = [];
  await check(
    "original cookie remains usable and exact save body/key survive a deliberate retry",
    async () => {
      const r = await request("/api/freight/customers", intent.body, {
        "X-Freight-Expected-Access": expected,
        "Idempotency-Key": intent.key,
      });
      assert.equal(r.status, 200);
      assert.equal(sourceCalls.length, 2);
      assert.equal(sourceCalls[1].path, "/v1/customers");
      assert.equal(sourceCalls[1].key, intent.key);
      assert.deepEqual(
        sourceCalls[1].body,
        Buffer.from(JSON.stringify(intent.body)),
      );
    },
  );
  await check(
    "nonredirected original document bytes and digest pass through",
    async () => {
      const r = await request("/api/freight/documents/proof/content");
      assert.equal(r.status, 200);
      assert.deepEqual(r.body, binary);
      assert.equal(r.headers.get("x-content-sha256"), sha(binary));
    },
  );
  await check(
    "nonredirected save recovery still seals and opens the exact original",
    async () => {
      const sealed = await request("/api/freight/save-journal", journal);
      assert.equal(sealed.status, 200);
      const opened = await request("/api/freight/save-journal", {
        action: "open",
        owner: journal.owner,
        sealed: JSON.parse(sealed.body).data,
      });
      assert.equal(opened.status, 200);
      assert.deepEqual(JSON.parse(opened.body).data, intent);
    },
  );
  child.kill("SIGTERM");
  await exit;
  child = null;
  report.result = report.checks.every((c) => c.result === "pass")
    ? "pass"
    : "failed";
} catch (error) {
  report.result = "failed";
  report.error = error.message;
} finally {
  if (child) {
    child.kill("SIGKILL");
    await once(child, "exit").catch(() => {});
  }
  await close(api);
  await close(sink);
  report.finished_at = new Date().toISOString();
  report.passed = report.checks.filter((c) => c.result === "pass").length;
  report.failed = report.checks.filter((c) => c.result === "failed").length;
  await outputFile.writeFile(JSON.stringify(report, null, 2) + "\n");
  await outputFile.close();
}
console.log(
  JSON.stringify({
    result: report.result,
    passed: report.passed,
    failed: report.failed,
    output: resolve(output),
  }),
);
process.exitCode = report.result === "pass" ? 0 : 1;
