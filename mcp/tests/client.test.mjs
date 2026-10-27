import test from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { once } from "node:events";
import { mkdtemp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  configuration,
  FreightClient,
  InputError,
  digest,
} from "../src/client.mjs";
const token = "owned-test-token-with-at-least-32-characters";
const ops = [
  { name: "read", method: "GET", path: "/v1/loads", body: "none" },
  { name: "write", method: "POST", path: "/v1/customers", body: "json" },
  { name: "entity", method: "GET", path: "/v1/loads/{id}", body: "none" },
  {
    name: "upload",
    method: "POST",
    path: "/v1/loads/{id}/documents",
    body: "binary",
  },
];
async function http(t, handler) {
  const server = createServer(handler);
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  t.after(() => {
    server.closeAllConnections();
    server.close();
  });
  return `http://127.0.0.1:${server.address().port}`;
}
const config = (url, extra = {}) =>
  configuration({
    FREIGHT_MCP_API_URL: url,
    FREIGHT_MCP_TOKEN: token,
    ...extra,
  });
const write = (body_json = "{}") => ({
  operation: "write",
  body_json,
  idempotency_key: "original-key",
});
for (const url of [
  "https://user:secret@example.com",
  "http://external.example",
  "file:///etc/passwd",
  "https://example.com/v1/",
  "https://example.com?token=secret",
  "https://example.com/#x",
])
  test(`configuration rejects unsafe API origin ${url.split(":")[0]}`, () =>
    assert.throws(() => config(url), InputError));
test("Clerk organization and mode are fixed configuration, without static fallback", () => {
  assert.throws(
    () => config("http://localhost", { FREIGHT_MCP_IDENTITY_MODE: "clerk" }),
    InputError,
  );
  assert.throws(
    () => config("http://localhost", { FREIGHT_MCP_ORGANIZATION_ID: "org_x" }),
    InputError,
  );
  assert.throws(
    () =>
      config("http://localhost", { FREIGHT_MCP_TOKEN_FILE: "/private/token" }),
    InputError,
  );
  assert.throws(
    () => config("http://localhost", { FREIGHT_MCP_TIMEOUT_MS: "NaN" }),
    InputError,
  );
});
test("exact original JSON and idempotency bytes reach one fixed-origin POST without rounding cents", async (t) => {
  let captured;
  const url = await http(t, async (req, res) => {
    const chunks = [];
    for await (const c of req) chunks.push(c);
    captured = {
      path: req.url,
      headers: req.headers,
      body: Buffer.concat(chunks),
    };
    res.setHeader("content-type", "application/json");
    res.end('{"cents":9007199254740993,"replayed":false}');
  });
  const raw = ' { "cents": 9007199254740993, "name": "🚚" }\n';
  const result = await new FreightClient(config(url), ops).request(
    "json",
    write(raw),
  );
  assert.equal(result.ok, true);
  assert.equal(captured.body.toString(), raw);
  assert.equal(result.request.body_sha256, digest(Buffer.from(raw)));
  assert.equal(captured.headers["idempotency-key"], "original-key");
  assert.equal(captured.headers.authorization, `Bearer ${token}`);
  assert.equal(captured.path, "/v1/customers");
  assert.equal(
    result.response.text,
    '{"cents":9007199254740993,"replayed":false}',
  );
});
test("redirect never forwards authorization or mutation to a second origin", async (t) => {
  let leaks = 0;
  const other = await http(t, (req, res) => {
    leaks++;
    res.end("leak");
  });
  const url = await http(t, (req, res) => {
    res.writeHead(307, { location: other });
    res.end();
  });
  const result = await new FreightClient(config(url), ops).request(
    "json",
    write(),
  );
  assert.equal(result.http_status, 307);
  assert.equal(result.outcome, "unconfirmed");
  assert.equal(leaks, 0);
});
test("dropped write response remains unconfirmed and is never automatically retried", async (t) => {
  let calls = 0;
  const url = await http(t, (req, res) => {
    calls++;
    req.socket.destroy();
  });
  const result = await new FreightClient(config(url), ops).request(
    "json",
    write(),
  );
  assert.equal(result.outcome, "unconfirmed");
  assert.equal(result.request.idempotency_key, "original-key");
  assert.equal(calls, 1);
  assert.equal(result.error.includes(token), false);
});
test("timeout and response overflow preserve uncertain-write evidence", async (t) => {
  const url = await http(t, (req, res) => {
    res.writeHead(200, { "content-type": "application/json" });
    res.write("x".repeat(2048));
  });
  const result = await new FreightClient(
    config(url, { FREIGHT_MCP_MAX_RESPONSE_BYTES: "1024" }),
    ops,
  ).request("json", write());
  assert.equal(result.outcome, "unconfirmed");
  assert.equal(result.request.body_sha256, digest(Buffer.from("{}")));
  const timed = await new FreightClient(
    config(url, { FREIGHT_MCP_TIMEOUT_MS: "10" }),
    ops,
  ).request("json", write());
  assert.equal(timed.outcome, "unconfirmed");
});
test("caller cancellation before dispatch does not send a write", async (t) => {
  let count = 0;
  const url = await http(t, (req, res) => {
    count++;
    res.end("{}");
  });
  const abort = new AbortController();
  abort.abort();
  const result = await new FreightClient(config(url), ops).request(
    "json",
    write(),
    abort.signal,
  );
  assert.equal(result.outcome, "not_sent");
  assert.equal(count, 0);
});
test("path/operation/key/body attacks fail before network I/O", async (t) => {
  let count = 0;
  const url = await http(t, (req, res) => {
    count++;
    res.end("{}");
  });
  const api = new FreightClient(config(url), ops);
  for (const [kind, args] of [
    ["none", { operation: "https://elsewhere" }],
    ["none", { operation: "entity", id: ".." }],
    ["none", { operation: "entity", id: "a%2fb" }],
    ["none", { operation: "read", id: "injected" }],
    ["json", { ...write(), idempotency_key: "x\r\ny" }],
    ["json", write("[]")],
    ["json", write("{")],
    ["json", write(JSON.stringify({ x: "x".repeat(1024 * 1024) }))],
  ])
    await assert.rejects(api.request(kind, args), InputError);
  assert.equal(count, 0);
});
test("rotating a credential file refreshes the token but never the configured organization", async (t) => {
  const dir = await mkdtemp(join(tmpdir(), "freight-mcp-token-"));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const file = join(dir, "token");
  const seen = [];
  const url = await http(t, (req, res) => {
    seen.push(req.headers);
    res.setHeader("content-type", "application/json");
    res.end("{}");
  });
  const api = new FreightClient(
    config(url, {
      FREIGHT_MCP_TOKEN: "",
      FREIGHT_MCP_TOKEN_FILE: file,
      FREIGHT_MCP_IDENTITY_MODE: "clerk",
      FREIGHT_MCP_ORGANIZATION_ID: "org_approved",
    }),
    ops,
  );
  assert.equal(
    (await api.request("none", { operation: "read" })).outcome,
    "not_sent",
  );
  await writeFile(file, token + "\n", { mode: 0o600 });
  await api.request("none", { operation: "read" });
  await writeFile(file, token + "-next");
  await api.request("none", { operation: "read" });
  assert.equal(seen.length, 2);
  assert.notEqual(seen[0].authorization, seen[1].authorization);
  assert.equal(seen[1]["x-freight-organization-id"], "org_approved");
  assert.equal(seen[1]["x-freight-identity-mode"], "clerk");
});
test("binary request and response preserve every original byte and digest", async (t) => {
  const bytes = Buffer.from([0, 255, 17, 13, 10, 128]);
  let captured;
  const url = await http(t, async (req, res) => {
    const a = [];
    for await (const c of req) a.push(c);
    captured = Buffer.concat(a);
    res.setHeader("content-type", "application/octet-stream");
    res.end(bytes);
  });
  const r = await new FreightClient(config(url), ops).request("binary", {
    operation: "upload",
    id: "ld_1",
    idempotency_key: "upload-key",
    body_base64: bytes.toString("base64"),
    content_type: "application/pdf",
  });
  assert.deepEqual(captured, bytes);
  assert.equal(r.response.base64, bytes.toString("base64"));
  assert.equal(r.response.sha256, digest(bytes));
});
test("API status and original conflict body survive as an MCP error, without an auth credential", async (t) => {
  const url = await http(t, (req, res) => {
    res.writeHead(409, { "content-type": "application/json" });
    res.end(JSON.stringify({ error: "changed " + token }));
  });
  const result = await new FreightClient(config(url), ops).request(
    "json",
    write(),
  );
  assert.equal(result.ok, false);
  assert.equal(result.http_status, 409);
  assert.equal(result.outcome, "unconfirmed");
  assert.equal(JSON.stringify(result).includes(token), false);
});
test("the complete 10 MiB canonical document crosses the binary boundary without regexp stack exhaustion", async (t) => {
  const bytes = Buffer.alloc(10 * 1024 * 1024, 120);
  let size = 0;
  const url = await http(t, async (req, res) => {
    for await (const chunk of req) size += chunk.length;
    res.setHeader("content-type", "application/json");
    res.end("{}");
  });
  const api = new FreightClient(config(url), ops);
  const input = {
    operation: "upload",
    id: "ld_full",
    idempotency_key: "full-original",
    body_base64: bytes.toString("base64"),
    content_type: "application/pdf",
  };
  assert.equal((await api.request("binary", input)).ok, true);
  assert.equal(size, bytes.length);
  await assert.rejects(
    api.request("binary", { ...input, body_base64: input.body_base64 + "\n" }),
    InputError,
  );
  await assert.rejects(
    api.request("binary", {
      ...input,
      body_base64: Buffer.alloc(10 * 1024 * 1024 + 1).toString("base64"),
    }),
    InputError,
  );
});
test("an oversized credential file is refused before any HTTP request", async (t) => {
  const dir = await mkdtemp(join(tmpdir(), "freight-mcp-invalid-token-"));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const file = join(dir, "token");
  await writeFile(file, "x".repeat(17000), { mode: 0o600 });
  let requests = 0;
  const url = await http(t, (req, res) => {
    requests++;
    res.end("{}");
  });
  const api = new FreightClient(
    config(url, { FREIGHT_MCP_TOKEN: "", FREIGHT_MCP_TOKEN_FILE: file }),
    ops,
  );
  assert.equal((await api.request("json", write())).outcome, "not_sent");
  assert.equal(requests, 0);
});
