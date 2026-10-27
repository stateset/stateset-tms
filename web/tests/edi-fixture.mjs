import serviceOrigins from "./service-origins.cjs";
const { fixturePort, fixtureOrigin, clerkOrigin } = serviceOrigins();
// Loopback-only, test-owned upstream. No real EDI traffic or credentials.
import { createServer } from "node:http";
import { clerkFixture } from "./clerk-fixture.mjs";
import { syncFixture } from "./sync-fixture.mjs";
const documents = new Map();
createServer(async (request, response) => {
  const pathname = new URL(request.url, "http://localhost").pathname;
  response.setHeader("Content-Type", "application/json");
  if (await clerkFixture(request, response)) return;
  if (await syncFixture(request, response)) return;
  if (request.method === "GET" && pathname === "/health")
    return response.end('{"ok":true}');
  if (
    request.method === "PUT" &&
    pathname.startsWith("/fixtures/") &&
    request.headers["x-fixture-key"] === "freight-fixture-only"
  ) {
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    documents.set(
      pathname.slice("/fixtures/".length),
      JSON.parse(Buffer.concat(chunks).toString()),
    );
    return response.end('{"ok":true}');
  }
  if (
    request.method === "GET" &&
    pathname.startsWith("/v1/documents/") &&
    request.headers.authorization === "Bearer freight-fixture-only"
  ) {
    const document = documents.get(pathname.slice("/v1/documents/".length));
    if (document)
      return response.end(JSON.stringify({ success: true, data: document }));
    response.statusCode = 404;
    return response.end('{"error":"No such fixture"}');
  }
  response.statusCode = 503;
  response.end('{"error":"Test upstream endpoint not configured"}');
}).listen(fixturePort, "127.0.0.1");
