import { cp } from "node:fs/promises";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
await cp(`${root}.next/static`, `${root}.next/standalone/.next/static`, {
  recursive: true,
});
const child = spawn(process.execPath, [`${root}.next/standalone/server.js`], {
  cwd: `${root}.next/standalone`,
  env: {
    ...process.env,
    PORT: process.env.PORT || "3010",
    HOSTNAME: process.env.FREIGHT_WEB_BIND || "127.0.0.1",
  },
  stdio: "inherit",
});
for (const signal of ["SIGINT", "SIGTERM"])
  process.on(signal, () => child.kill(signal));
child.on("error", (error) => {
  console.error(error.message);
  process.exit(1);
});
child.on("exit", (code) => process.exit(code ?? 0));
