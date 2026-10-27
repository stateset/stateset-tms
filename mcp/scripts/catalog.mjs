// Check in the reviewed API surface. Runtime never parses Rust or accepts URLs.
import { readFile, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
const root = new URL("../../", import.meta.url);
const source = await readFile(new URL("src/api.rs", root), "utf8");
const docs = await readFile(new URL("docs/api.md", root), "utf8");
const routes = [];
for (const match of source.matchAll(/\.route\(/g)) {
  let i = match.index + match[0].length,
    depth = 1,
    quoted = false,
    escaped = false;
  const start = i;
  for (; i < source.length && depth; i++) {
    const c = source[i];
    if (quoted) {
      if (escaped) escaped = false;
      else if (c === "\\") escaped = true;
      else if (c === '"') quoted = false;
    } else if (c === '"') quoted = true;
    else if (c === "(") depth++;
    else if (c === ")") depth--;
  }
  if (depth) throw new Error("Unbalanced router source");
  const expression = source.slice(start, i - 1);
  const path = expression.match(/^\s*"([^"]+)"/)?.[1];
  if (!path?.startsWith("/v1/")) continue;
  const methods = [...expression.matchAll(/\b(get|post)\(([\w:]+)\)/g)];
  if (
    !methods.length ||
    expression
      .replace(/^\s*"[^"]+"\s*,/, "")
      .replace(/\.layer\(DefaultBodyLimit::max\(MAX_DOCUMENT_BYTES\)\)/g, "")
      .replace(/\b(get|post)\([\w:]+\)/g, "")
      .replace(/[\s.,]/g, "")
  )
    throw new Error(`Unrecognized protected route: ${path}`);
  for (const [, method, handler] of methods) {
    const name = handler.replaceAll("::", "_");
    const row = docs
      .split("\n")
      .find(
        (line) =>
          line.startsWith("|") &&
          line.includes("`" + path) &&
          new RegExp(`\\|\\s*${method.toUpperCase()}\\s*\\|`).test(line),
      );
    const description =
      row?.split("|").slice(3, -1).join("|").trim() ||
      `${method.toUpperCase()} ${path}; existing Freight API validation and authorization apply.`;
    routes.push({
      name,
      method: method.toUpperCase(),
      path,
      body:
        method === "get"
          ? "none"
          : ["upload_document", "partner_pod"].includes(handler)
            ? "binary"
            : "json",
      description,
    });
  }
}
routes.sort((a, b) => a.name.localeCompare(b.name, "en"));
if (new Set(routes.map((r) => r.name)).size !== routes.length)
  throw new Error("Duplicate operation names");
const result =
  JSON.stringify(
    {
      version: 1,
      api_source_sha256: createHash("sha256").update(source).digest("hex"),
      operations: routes,
    },
    null,
    2,
  ) + "\n";
const output = new URL("../catalog.json", import.meta.url);
if (process.argv.includes("--check")) {
  if ((await readFile(output, "utf8")) !== result)
    throw new Error(
      "MCP catalog differs from registered API/docs; regenerate and review it",
    );
} else await writeFile(output, result);
const guideNames = new Set(["api.md", "save-resolution.md"]);
for (const match of docs.matchAll(/\]\(([a-z0-9-]+\.md)(?:#[^)]*)?\)/g))
  guideNames.add(match[1]);
const guides = {};
for (const file of [...guideNames].sort())
  guides[file.slice(0, -3)] = await readFile(
    new URL("docs/" + file, root),
    "utf8",
  );
const guideOutput = new URL("../guides.json", import.meta.url);
const guideText = JSON.stringify(guides, null, 2) + "\n";
if (process.argv.includes("--check")) {
  if ((await readFile(guideOutput, "utf8")) !== guideText)
    throw new Error("MCP API guide bundle is stale");
} else await writeFile(guideOutput, guideText);
console.error(
  `Verified ${routes.length} protected API operations and ${guideNames.size} guides`,
);
