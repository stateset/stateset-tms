import { open } from "node:fs/promises";
import { createHash } from "node:crypto";

export const digest = (bytes) =>
  createHash("sha256").update(bytes).digest("hex");
export class InputError extends Error {}
const fail = (message) => {
  throw new InputError(message);
};

export function configuration(env = process.env) {
  let url;
  try {
    url = new URL(env.FREIGHT_MCP_API_URL);
  } catch {
    fail("FREIGHT_MCP_API_URL must be the Freight API origin");
  }
  if (
    url.username ||
    url.password ||
    url.search ||
    url.hash ||
    url.pathname !== "/"
  )
    fail("The API URL must be an origin without credentials, query, or path");
  const local = ["localhost", "127.0.0.1", "[::1]"].includes(url.hostname);
  if (
    url.protocol !== "https:" &&
    !(
      url.protocol === "http:" &&
      (local || env.FREIGHT_MCP_ALLOW_HTTP === "true")
    )
  )
    fail(
      "Use HTTPS, loopback HTTP, or explicitly configure FREIGHT_MCP_ALLOW_HTTP=true for a trusted private network",
    );
  if (!!env.FREIGHT_MCP_TOKEN === !!env.FREIGHT_MCP_TOKEN_FILE)
    fail(
      "Configure exactly one of FREIGHT_MCP_TOKEN or FREIGHT_MCP_TOKEN_FILE",
    );
  const mode = env.FREIGHT_MCP_IDENTITY_MODE || "credential";
  if (!["credential", "clerk"].includes(mode))
    fail("Identity mode must be credential or clerk");
  const organization = env.FREIGHT_MCP_ORGANIZATION_ID;
  if (
    mode === "clerk" &&
    !/^org_[A-Za-z0-9_-]{1,180}$/.test(organization || "")
  )
    fail("Clerk mode requires a fixed FREIGHT_MCP_ORGANIZATION_ID");
  if (mode !== "clerk" && organization)
    fail("Organization binding requires Clerk mode");
  function integer(name, fallback, min, max) {
    const value = env[name] ?? String(fallback);
    if (!/^\d+$/.test(value) || +value < min || +value > max)
      fail(`${name} is outside its supported range`);
    return +value;
  }
  return Object.freeze({
    origin: url.origin,
    token: env.FREIGHT_MCP_TOKEN,
    tokenFile: env.FREIGHT_MCP_TOKEN_FILE,
    mode,
    organization,
    timeout: integer("FREIGHT_MCP_TIMEOUT_MS", 30000, 1, 120000),
    maxResponse: integer(
      "FREIGHT_MCP_MAX_RESPONSE_BYTES",
      16 * 1024 * 1024,
      1024,
      64 * 1024 * 1024,
    ),
  });
}

export class FreightClient {
  constructor(config, operations) {
    this.config = config;
    this.operations = new Map(operations.map((op) => [op.name, op]));
  }
  async request(kind, input, signal) {
    const op = this.operations.get(input.operation);
    if (!op || op.body !== kind) fail("Unknown operation for this tool");
    const hasId = op.path.includes("{id}");
    if (hasId !== (input.id !== undefined))
      fail("Supply id only for an operation with an entity path");
    if (hasId && !/^[A-Za-z0-9_-]{1,200}$/.test(input.id))
      fail(
        "Entity id must contain 1–200 letters, digits, underscores or hyphens",
      );
    const path = hasId ? op.path.replace("{id}", input.id) : op.path;
    const url = new URL(path, this.config.origin);
    if (input.query) {
      if (Object.keys(input.query).length > 32)
        fail("Too many query parameters");
      for (const [key, value] of Object.entries(input.query)) {
        if (
          !/^[a-z_]{1,80}$/.test(key) ||
          typeof value !== "string" ||
          Buffer.byteLength(value) > 4096
        )
          fail("Invalid query parameter");
        url.searchParams.set(key, value);
      }
    }
    const headers = new Headers();
    let body;
    if (kind !== "none") {
      if (!/^[\x20-\x7e]{1,200}$/.test(input.idempotency_key || ""))
        fail(
          "An original printable Idempotency-Key of 1–200 characters is required",
        );
      headers.set("Idempotency-Key", input.idempotency_key);
      if (kind === "json") {
        if (typeof input.body_json !== "string")
          fail("body_json must contain the original JSON object text");
        let parsed;
        try {
          parsed = JSON.parse(input.body_json);
        } catch {
          fail("body_json must be valid JSON");
        }
        if (!parsed || typeof parsed !== "object" || Array.isArray(parsed))
          fail("body_json must be an object");
        body = Buffer.from(input.body_json);
        headers.set("Content-Type", "application/json");
      } else {
        if (typeof input.body_base64 !== "string")
          fail("body_base64 must be canonical base64");
        body = Buffer.from(input.body_base64, "base64");
        if (body.toString("base64") !== input.body_base64)
          fail("body_base64 must be canonical base64");
        if (
          !/^[A-Za-z0-9!#$&^_.+-]+\/[A-Za-z0-9!#$&^_.+-]+$/.test(
            input.content_type || "",
          )
        )
          fail("A plain media type is required");
        headers.set("Content-Type", input.content_type);
      }
      if (body.length > (kind === "binary" ? 10 : 1) * 1024 * 1024)
        fail(
          "Request exceeds Freight's body limit: JSON 1 MiB, document 10 MiB",
        );
    }
    const request = {
      operation: op.name,
      method: op.method,
      path: url.pathname + url.search,
      ...(body
        ? { idempotency_key: input.idempotency_key, body_sha256: digest(body) }
        : {}),
    };
    const unconfirmed = kind !== "none" ? "unconfirmed" : "unavailable";
    let token;
    try {
      token = this.config.token;
      if (this.config.tokenFile) {
        const file = await open(this.config.tokenFile, "r");
        try {
          const metadata = await file.stat();
          if (!metadata.isFile() || metadata.size > 16386)
            throw new Error("Invalid credential file");
          const buffer = Buffer.alloc(16387);
          const { bytesRead } = await file.read(buffer, 0, buffer.length, 0);
          token = buffer.subarray(0, bytesRead).toString("utf8").trim();
        } finally {
          await file.close();
        }
      }
    } catch {
      return {
        ok: false,
        outcome: "not_sent",
        request,
        error: "Credential file could not be read",
      };
    }
    if (typeof token !== "string" || !/^[\x21-\x7e]{1,16384}$/.test(token))
      return {
        ok: false,
        outcome: "not_sent",
        request,
        error: "Credential is missing or invalid",
      };
    headers.set("Authorization", `Bearer ${token}`);
    if (this.config.mode === "clerk") {
      headers.set("X-Freight-Identity-Mode", "clerk");
      headers.set("X-Freight-Organization-Id", this.config.organization);
    }
    const combined = AbortSignal.any([
      AbortSignal.timeout(this.config.timeout),
      ...(signal ? [signal] : []),
    ]);
    try {
      if (combined.aborted)
        return {
          ok: false,
          outcome: "not_sent",
          request,
          error: "Request cancelled before dispatch",
        };
      const response = await fetch(url, {
        method: op.method,
        headers,
        body,
        redirect: "manual",
        signal: combined,
      });
      if (response.status >= 300 && response.status < 400) {
        await response.body?.cancel();
        return {
          ok: false,
          outcome: unconfirmed,
          http_status: response.status,
          request,
          error: "API redirect refused",
        };
      }
      const chunks = [];
      let length = 0;
      for await (const chunk of response.body || []) {
        length += chunk.length;
        if (length > this.config.maxResponse)
          throw new Error("response budget");
        chunks.push(chunk);
      }
      const bytes = Buffer.concat(chunks),
        type =
          response.headers.get("content-type") || "application/octet-stream";
      const text = /^(application\/(?:json|[^;]+\+json)|text\/)/i.test(type);
      const result = {
        ok: response.ok,
        outcome: response.ok ? "response_received" : unconfirmed,
        http_status: response.status,
        request,
        response: {
          content_type: type,
          bytes: bytes.length,
          sha256: digest(bytes),
          ...(text
            ? { text: bytes.toString("utf8") }
            : { base64: bytes.toString("base64") }),
          ...(/^[a-f0-9]{64}$/i.test(
            response.headers.get("x-content-sha256") || "",
          )
            ? { original_sha256: response.headers.get("x-content-sha256") }
            : {}),
        },
      };
      // Preserve JSON as text: parsing/re-serializing it could round i64 cents.
      // Never echo an authorization credential in an upstream error result.
      if (!response.ok && result.response.text?.includes(token)) {
        result.response.text = result.response.text.replaceAll(
          token,
          "[credential redacted]",
        );
        result.response.credential_redacted = true;
        const shown = Buffer.from(result.response.text);
        result.response.bytes = shown.length;
        result.response.sha256 = digest(shown);
      }
      return result;
    } catch {
      return {
        ok: false,
        outcome: unconfirmed,
        request,
        error:
          "No complete API response: connection, cancellation, timeout, or response limit. A write may have committed. Recover or replay the exact original key, path and bytes; do not create a replacement key.",
      };
    }
  }
}
