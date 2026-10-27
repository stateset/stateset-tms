import test from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { fileURLToPath } from "node:url";
import { Client } from "@modelcontextprotocol/client";
import { StdioClientTransport } from "@modelcontextprotocol/client/stdio";
const executable = fileURLToPath(new URL("../src/index.mjs", import.meta.url));
const env = {
  PATH: process.env.PATH,
  FREIGHT_MCP_API_URL: "http://127.0.0.1:1",
  FREIGHT_MCP_TOKEN: "owned-lifecycle-token-at-least-32-characters",
};
test("stdin EOF exits the actual entry point promptly with an empty protocol channel", async () => {
  const child = spawn(process.execPath, [executable], {
    env,
    stdio: ["pipe", "pipe", "pipe"],
  });
  let out = "",
    err = "";
  child.stdout.on("data", (v) => (out += v));
  child.stderr.on("data", (v) => (err += v));
  const terminal = once(child, "exit");
  child.stdin.end();
  const timer = setTimeout(() => child.kill("SIGKILL"), 3000);
  const [code, signal] = await terminal;
  clearTimeout(timer);
  assert.equal(code, 0);
  assert.equal(signal, null);
  assert.equal(out, "");
  assert.equal(err, "");
});
test("startup rejects embedded origin credentials without printing their values", async () => {
  const secret = "never-echo-this-origin-secret";
  const child = spawn(process.execPath, [executable], {
    env: { ...env, FREIGHT_MCP_API_URL: `https://user:${secret}@example.com` },
    stdio: ["pipe", "pipe", "pipe"],
  });
  let out = "",
    err = "";
  child.stdout.on("data", (v) => (out += v));
  child.stderr.on("data", (v) => (err += v));
  const terminal = once(child, "exit");
  child.stdin.end();
  const [code] = await terminal;
  assert.equal(code, 1);
  assert.equal(out, "");
  assert.equal(err.includes(secret), false);
  assert.equal(err.includes(env.FREIGHT_MCP_TOKEN), false);
});
test("SIGTERM closes an initialized SDK connection and releases the MCP process", async (t) => {
  const transport = new StdioClientTransport({
    command: process.execPath,
    args: [executable],
    env,
    stderr: "pipe",
  });
  const client = new Client(
    { name: "lifecycle", version: "1.0.0" },
    { versionNegotiation: { mode: "legacy" } },
  );
  t.after(() => client.close());
  await client.connect(transport);
  assert.equal((await client.listTools()).tools.length, 4);
  const pid = transport.pid;
  const previous = transport.onclose;
  const closed = new Promise((resolve) => {
    transport.onclose = () => {
      previous?.();
      resolve();
    };
  });
  process.kill(pid, "SIGTERM");
  let timer;
  try {
    await Promise.race([
      closed,
      new Promise((_, reject) => {
        timer = setTimeout(() => {
          try {
            process.kill(pid, "SIGKILL");
          } catch {}
          reject(Error("MCP did not close after SIGTERM"));
        }, 3000);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
  assert.throws(() => process.kill(pid, 0), { code: "ESRCH" });
});
