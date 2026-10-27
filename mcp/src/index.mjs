#!/usr/bin/env node
import {
  serveStdio,
  StdioServerTransport,
} from "@modelcontextprotocol/server/stdio";
import { createServer } from "./server.mjs";
import { configuration, InputError } from "./client.mjs";

try {
  const config = configuration();
  const handle = serveStdio(() => createServer(config), {
    transport: new StdioServerTransport(process.stdin, process.stdout, {
      maxBufferSize: 32 * 1024 * 1024,
    }),
    onerror: () => console.error("Freight MCP protocol error"),
  });
  process.once("SIGINT", () => {
    void handle.close();
  });
  process.once("SIGTERM", () => {
    void handle.close();
  });
} catch (error) {
  console.error(
    error instanceof InputError ? error.message : "Freight MCP could not start",
  );
  process.exitCode = 1;
}
