import test, { before, after } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createServer as httpServer } from "node:http";
import { createServer as tcpServer } from "node:net";
import { once } from "node:events";
import {
  mkdtemp,
  readFile,
  writeFile,
  open,
  rm,
  access,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { randomUUID } from "node:crypto";
import { Client } from "@modelcontextprotocol/client";
import { StdioClientTransport } from "@modelcontextprotocol/client/stdio";
const root = fileURLToPath(new URL("../../", import.meta.url));
const executable = fileURLToPath(new URL("../src/index.mjs", import.meta.url));
const tokens = Object.fromEntries(
  ["operator", "viewer", "finance", "other", "carrier", "customer"].map((n) => [
    n,
    "mcp-owned-fixture-" + n + "-".repeat(40),
  ]),
);
const clients = [],
  children = [];
let dir,
  base,
  apiURL,
  fixtureURL,
  clerkOrigin,
  logs = [],
  apiProcess,
  apiEnvironment,
  apiExecutable;
async function portBlock() {
  for (let n = 0; n < 100; n++) {
    const first = tcpServer();
    first.listen(0, "127.0.0.1");
    await once(first, "listening");
    const port = first.address().port;
    await new Promise((r) => first.close(r));
    if (port > 65532) continue;
    const probes = [];
    let ok = true;
    try {
      for (let i = 0; i < 4; i++) {
        const p = tcpServer();
        probes.push(p);
        p.listen(port + i, "127.0.0.1");
        await once(p, "listening");
      }
    } catch {
      ok = false;
    } finally {
      await Promise.all(probes.map((s) => new Promise((r) => s.close(r))));
    }
    if (ok) return port;
  }
  throw Error("No isolated port block");
}
async function start(command, args, env, name) {
  const log = await open(join(dir, name + ".log"), "wx", 0o600);
  logs.push(log);
  const child = spawn(command, args, {
    cwd: dir,
    env: { PATH: process.env.PATH, LANG: "C.UTF-8", ...env },
    stdio: ["ignore", log.fd, log.fd],
  });
  children.push(child);
  await once(child, "spawn");
  return child;
}
async function ready(url, child) {
  const end = Date.now() + 20000;
  while (Date.now() < end) {
    if (child.exitCode !== null)
      throw Error("Owned service exited before readiness");
    try {
      if ((await fetch(url)).ok) return;
    } catch {}
    await new Promise((r) => setTimeout(r, 40));
  }
  throw Error("Owned service readiness expired");
}
async function connect(t, token = tokens.operator, options = {}) {
  const image = process.env.FREIGHT_MCP_TEST_IMAGE;
  const dockerArgs = [
    "--host",
    "unix:///var/run/docker.sock",
    "run",
    "--rm",
    "-i",
    "--read-only",
    "--network",
    "host",
    "--cap-drop",
    "ALL",
    "--security-opt",
    "no-new-privileges",
    "--memory",
    "256m",
    "--memory-swap",
    "256m",
    "--cpus",
    "2",
    "--label",
    "com.stateset.freight.runtime=freight-mcp-" + randomUUID(),
    ...[
      "FREIGHT_MCP_API_URL",
      "FREIGHT_MCP_TOKEN",
      "FREIGHT_MCP_IDENTITY_MODE",
      "FREIGHT_MCP_ORGANIZATION_ID",
    ].flatMap((name) => ["-e", name]),
    image,
  ];
  const transport = new StdioClientTransport({
    command: image ? "docker" : process.execPath,
    args: image ? dockerArgs : [executable],
    cwd: dir,
    maxBufferSize: 64 * 1024 * 1024,
    env: {
      PATH: process.env.PATH,
      FREIGHT_MCP_API_URL: options.url || apiURL,
      FREIGHT_MCP_TOKEN: token,
      ...options.env,
    },
    stderr: "pipe",
  });
  let stderr = "";
  transport.stderr.on("data", (x) => {
    stderr += x;
  });
  const client = new Client(
    { name: "freight-qualification", version: "1.0.0" },
    {
      versionNegotiation: options.legacy
        ? { mode: "legacy" }
        : { mode: { pin: "2026-07-28" } },
    },
  );
  clients.push(client);
  t.after(async () => {
    await client.close();
    assert.equal(stderr.includes(token), false);
  });
  await client.connect(transport);
  return client;
}
async function call(client, name, args) {
  const r = await client.callTool({ name, arguments: args });
  const v = r.structuredContent;
  assert.deepEqual(JSON.parse(r.content[0].text), v);
  assert.equal(!!r.isError, !v.ok);
  return v;
}
const read = (client, operation = "workspace", rest = {}) =>
  call(client, "freight_read", { operation, ...rest });
const write = (client, operation, body, rest = {}) =>
  call(client, "freight_write", {
    operation,
    idempotency_key: randomUUID(),
    body_json: JSON.stringify(body),
    ...rest,
  });
const data = (r) => JSON.parse(r.response.text);
async function control(body) {
  const r = await fetch(fixtureURL + "/__identity/control", {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "x-fixture-key": "freight-fixture-only",
    },
    body: JSON.stringify(body),
  });
  assert.equal(r.ok, true);
}
async function jwt(org = "org_staff") {
  const r = await fetch(fixtureURL + "/__identity/token", {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "x-fixture-key": "freight-fixture-only",
    },
    body: JSON.stringify({
      o: { id: org, rol: "admin", slg: "untrusted" },
      sid: "sess_staff",
    }),
  });
  assert.equal(r.ok, true);
  return (await r.json()).token;
}
before(async () => {
  dir =
    process.env.FREIGHT_MCP_TEST_ARTIFACTS ||
    (await mkdtemp(join(tmpdir(), "freight-mcp-integration-")));
  base = process.env.FREIGHT_MCP_TEST_PORT_BASE
    ? +process.env.FREIGHT_MCP_TEST_PORT_BASE
    : await portBlock();
  apiURL = `http://127.0.0.1:${base + 2}`;
  fixtureURL = `http://127.0.0.1:${base + 3}`;
  clerkOrigin = `http://127.0.0.1:${base + 1}`;
  const grants = {};
  for (const [name, key] of Object.entries(tokens))
    grants[key] = {
      tenant_id: name === "other" ? "mcp_other" : "demo",
      role: name === "operator" || name === "other" ? "operations" : name,
      subject: "mcp-" + name,
      ...(["carrier", "customer"].includes(name)
        ? { counterparty_id: name === "carrier" ? "ca4" : "c1" }
        : {}),
    };
  const fixture = await start(
    process.execPath,
    [join(root, "web/tests/edi-fixture.mjs")],
    { FREIGHT_E2E_PORT_BASE: String(base) },
    "clerk-fixture",
  );
  await ready(fixtureURL + "/health", fixture);
  const apiBinary =
    process.env.FREIGHT_MCP_TEST_API_BINARY ||
    join(root, "target/release/stateset-freight");
  await access(apiBinary);
  apiExecutable = apiBinary;
  apiEnvironment = {
    FREIGHT_BIND_ADDR: `127.0.0.1:${base + 2}`,
    FREIGHT_DEMO: "true",
    DATABASE_URL: process.env.FREIGHT_MCP_TEST_DATABASE_URL || "",
    FREIGHT_MIGRATIONS:
      process.env.FREIGHT_MCP_TEST_MIGRATIONS ||
      (process.env.FREIGHT_MCP_TEST_DATABASE_URL ? "verify" : "apply"),
    FREIGHT_API_KEYS_JSON: JSON.stringify(grants),
    FREIGHT_WORKSPACE_CONCURRENCY: "1",
    TOKIO_WORKER_THREADS: "2",
    RUST_LOG: "warn",
    FREIGHT_CLERK_JSON: JSON.stringify({
      issuer: fixtureURL,
      secret_key: "sk_test_owned_fixture_only_at_least_32_characters",
      authorized_parties: [clerkOrigin],
      fixture_api_url: fixtureURL,
      organizations: {
        org_staff: {
          tenant_id: "mcp_identity",
          roles: { "org:operations": "operations", "org:viewer": "viewer" },
        },
        org_other: {
          tenant_id: "mcp_other",
          roles: { "org:operations": "operations" },
        },
      },
    }),
  };
  apiProcess = await start(apiBinary, [], apiEnvironment, "api");
  await ready(apiURL + "/health", apiProcess);
});
after(async () => {
  for (const client of clients) await client.close();
  for (const child of children) {
    if (child.exitCode === null) {
      const ended = once(child, "exit");
      child.kill("SIGTERM");
      await Promise.race([
        ended,
        new Promise((_, rej) =>
          setTimeout(() => {
            child.kill("SIGKILL");
            rej(Error("Owned service did not drain"));
          }, 15000).unref(),
        ),
      ]);
    }
  }
  for (const log of logs) await log.close();
  console.error("Owned MCP diagnostics retained at " + dir);
});
for (const legacy of [false, true])
  test(`real SDK stdio ${legacy ? "legacy2025" : "2026-07-28"} discovers all operations, guides and authenticates`, async (t) => {
    const client = await connect(t, tokens.operator, { legacy });
    const tools = await client.listTools();
    assert.deepEqual(
      tools.tools.map((v) => v.name),
      ["freight_catalog", "freight_read", "freight_write", "freight_upload"],
    );
    assert.equal(client.getServerVersion().name, "stateset-freight");
    const list = await call(client, "freight_catalog", {});
    assert.equal(list.operations.length, 129);
    assert.equal(new Set(list.operations.map((v) => v.name)).size, 129);
    assert.equal(
      list.operations.some((v) => v.path === "/metrics"),
      false,
    );
    const resources = await client.listResources();
    assert.ok(resources.resources.length >= 26);
    const guide = await client.readResource({ uri: "freight://guides/api" });
    assert.match(guide.contents[0].text, /integer cents/);
    const r = await read(client, "access");
    assert.equal(r.http_status, 200);
    assert.equal(data(r).access.role, "operations");
    assert.equal(data(r).tenant_id, "demo");
  });
test("console pages are discoverable bounded staff reads through the real MCP adapter", async (t) => {
  const viewer = await connect(t, tokens.viewer);
  assert.equal(data(await read(viewer, "access")).features.console_catalog, true);
  for (const view of ["billing", "payables", "integrations"]) {
    const result = await read(viewer, "console_page", { query: { view, limit: "1" } });
    assert.equal(result.http_status, 200);
    const page = data(result);
    assert.equal(page.view, view);
    assert.equal(page.limit, 1);
    assert.equal(page.totals_scope, "workspace");
    assert.ok(["bills", "settlements", "outbox", "inbox"].reduce((n, key) => n + page[key].length, 0) <= 1);
  }
  const carrier = await connect(t, tokens.carrier);
  assert.equal((await read(carrier, "console_page", { query: { view: "billing" } })).http_status, 403);
  assert.equal((await read(viewer, "console_page", { query: { view: "billing", direction: "all" } })).http_status, 422);
});
test("real API commits once, replays the original receipt, and rejects an edited key reuse", async (t) => {
  const c = await connect(t);
  const key = randomUUID(),
    body = {
      name: "MCP original " + randomUUID(),
      contact: "original contact",
    },
    a = { idempotency_key: key };
  const first = await write(c, "create_customer", body, a);
  assert.equal(first.ok, true);
  assert.equal(data(first).replayed, false);
  const replay = await write(c, "create_customer", body, a);
  assert.equal(data(replay).replayed, true);
  assert.deepEqual(data(replay).data, data(first).data);
  const changed = await write(
    c,
    "create_customer",
    { ...body, name: "edited" },
    a,
  );
  assert.equal(changed.http_status, 409);
  const w = data(await read(c));
  assert.equal(w.customers.filter((v) => v.name === body.name).length, 1);
});
test("actual tenant/role and partner boundaries cannot be selected by tool arguments", async (t) => {
  const viewer = await connect(t, tokens.viewer),
    operator = await connect(t),
    other = await connect(t, tokens.other),
    carrier = await connect(t, tokens.carrier),
    customer = await connect(t, tokens.customer);
  const r = await write(viewer, "create_customer", {
    name: "forbidden-" + randomUUID(),
    contact: "x",
  });
  assert.equal(r.http_status, 403);
  const otherName = "other-" + randomUUID();
  assert.equal(
    (await write(other, "create_customer", { name: otherName, contact: "x" }))
      .ok,
    true,
  );
  assert.equal(
    data(await read(operator)).customers.some((v) => v.name === otherName),
    false,
  );
  assert.equal((await read(carrier)).http_status, 403);
  assert.equal(data(await read(carrier, "partner_workspace")).kind, "carrier");
  assert.equal(
    data(await read(customer, "partner_workspace")).kind,
    "customer",
  );
  const denied = await write(
    operator,
    "approve_invoice",
    {},
    { id: "not_an_invoice" },
  );
  assert.equal(denied.http_status, 403);
  const injection = await operator.callTool({
    name: "freight_read",
    arguments: {
      operation: "workspace",
      tenant_id: "mcp_other",
      headers: { authorization: "bad" },
    },
  });
  assert.equal(injection.isError, true);
});
test("registered operations reject stale shipment reviews and retain actual API 422 details", async (t) => {
  const c = await connect(t);
  const w = data(await read(c));
  const load = w.loads.find((v) => v.status === "Planned") || w.loads[0];
  assert.ok(load);
  const stale = await write(
    c,
    "command",
    {
      action: "cancel",
      expected_version: load.version + 999,
      note: "stale MCP review",
    },
    { id: load.id },
  );
  assert.equal(stale.http_status, 409);
  assert.match(stale.response.text, /changed|version/i);
  const invalid = await write(c, "create_load", { reference: "INCOMPLETE" });
  assert.equal(invalid.http_status, 422);
  assert.equal(
    data(await read(c)).loads.find((v) => v.id === load.id).version,
    load.version,
  );
});
test("actual original document upload, replay and download retain exact PDF bytes", async (t) => {
  const c = await connect(t);
  const w = data(await read(c));
  const load = w.loads[0];
  const bytes = Buffer.concat([
    Buffer.from("%PDF-1.4\n"),
    Buffer.alloc(10 * 1024 * 1024 - 16, 120),
    Buffer.from("\n%%EOF\n"),
  ]);
  assert.equal(bytes.length, 10 * 1024 * 1024);
  const args = {
    operation: "upload_document",
    id: load.id,
    idempotency_key: randomUUID(),
    query: { kind: "pod", filename: "mcp-owned.pdf" },
    content_type: "application/pdf",
    body_base64: bytes.toString("base64"),
  };
  const uploaded = await call(c, "freight_upload", args);
  assert.equal(uploaded.ok, true, uploaded.response?.text);
  const record = data(uploaded).data;
  const repeated = await call(c, "freight_upload", args);
  assert.equal(data(repeated).replayed, true);
  const downloaded = await read(c, "document_content", { id: record.id });
  assert.equal(downloaded.ok, true);
  assert.equal(downloaded.response.base64, bytes.toString("base64"));
  assert.equal(downloaded.response.sha256, uploaded.request.body_sha256);
});
test("a dropped response after real commit is unconfirmed; explicit original replay recovers without duplication", async (t) => {
  let writes = 0;
  const server = httpServer(async (req, res) => {
    const chunks = [];
    for await (const b of req) chunks.push(b);
    const bytes = Buffer.concat(chunks);
    const headers = {
      authorization: req.headers.authorization,
      "content-type": req.headers["content-type"] || "application/json",
      ...(req.headers["idempotency-key"]
        ? { "idempotency-key": req.headers["idempotency-key"] }
        : {}),
    };
    const upstream = await fetch(apiURL + req.url, {
      method: req.method,
      headers,
      ...(req.method === "POST" ? { body: bytes } : {}),
    });
    const body = Buffer.from(await upstream.arrayBuffer());
    if (req.method === "POST" && ++writes === 1) {
      assert.equal(upstream.status, 200);
      req.socket.destroy();
    } else {
      res.writeHead(upstream.status, {
        "content-type": upstream.headers.get("content-type"),
      });
      res.end(body);
    }
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  t.after(() => {
    server.closeAllConnections();
    server.close();
  });
  const c = await connect(t, tokens.operator, {
    url: `http://127.0.0.1:${server.address().port}`,
  });
  const body = {
      name: "lost-response-" + randomUUID(),
      contact: "retained original",
    },
    options = { idempotency_key: randomUUID() };
  const lost = await write(c, "create_customer", body, options);
  assert.equal(lost.outcome, "unconfirmed");
  assert.equal(writes, 1);
  await c.close();
  if (process.env.FREIGHT_MCP_TEST_DATABASE_URL) {
    const ended = once(apiProcess, "exit");
    apiProcess.kill("SIGTERM");
    await ended;
    apiProcess = await start(apiExecutable, [], apiEnvironment, "api-restart");
    await ready(apiURL + "/health", apiProcess);
  }
  const replacement = await connect(t, tokens.operator, {
    url: `http://127.0.0.1:${server.address().port}`,
  });
  const replay = await write(replacement, "create_customer", body, options);
  assert.equal(data(replay).replayed, true);
  assert.equal(writes, 2);
  assert.equal(
    data(await read(replacement)).customers.filter((v) => v.name === body.name)
      .length,
    1,
  );
});
test("MCP original-save resolution durably fences a delayed unsupported attempt before application", async (t) => {
  const c = await connect(t);
  const key = randomUUID(),
    body = { name: "stopped-" + randomUUID(), contact: "original" };
  const resolved = await write(c, "resolve_save", {
    request_key: key,
    path: "customers",
    body,
    note: "Operator stopped the unsubmitted original",
  });
  assert.equal(resolved.ok, true, resolved.response?.text);
  const delayed = await write(c, "create_customer", body, {
    idempotency_key: key,
  });
  assert.equal(delayed.http_status, 409);
  assert.equal(
    data(await read(c)).customers.some((v) => v.name === body.name),
    false,
  );
});
test("Clerk membership, configured organization, revocation and provider outage stay live through MCP", async (t) => {
  await control({
    status: "active",
    role: "org:operations",
    member: true,
    org: "org_staff",
    outage: false,
  });
  const token = await jwt();
  const c = await connect(t, token, {
    env: {
      FREIGHT_MCP_IDENTITY_MODE: "clerk",
      FREIGHT_MCP_ORGANIZATION_ID: "org_staff",
    },
  });
  assert.equal(data(await read(c, "access")).access.role, "operations");
  await control({ role: "org:viewer" });
  assert.equal(
    (await write(c, "create_customer", { name: "role-stale", contact: "x" }))
      .http_status,
    403,
  );
  await control({ status: "revoked" });
  assert.equal((await read(c, "access")).http_status, 401);
  await control({ status: "active", role: "org:operations", outage: true });
  assert.equal((await read(c, "access")).http_status, 503);
  await control({ outage: false });
  const mismatch = await connect(t, token, {
    env: {
      FREIGHT_MCP_IDENTITY_MODE: "clerk",
      FREIGHT_MCP_ORGANIZATION_ID: "org_other",
    },
  });
  assert.equal((await read(mismatch, "access")).http_status, 403);
  const fallback = await connect(t, tokens.operator, {
    env: {
      FREIGHT_MCP_IDENTITY_MODE: "clerk",
      FREIGHT_MCP_ORGANIZATION_ID: "org_staff",
    },
  });
  assert.equal((await read(fallback, "access")).http_status, 401);
});
