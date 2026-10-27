import test from "node:test";
import assert from "node:assert/strict";
import serviceOrigins from "./service-origins.cjs";
import { createRequire } from "node:module";

test("owned service defaults preserve existing browser and identity origins", () => {
  const origins = serviceOrigins("");
  assert.equal(origins.staffOrigin, "http://127.0.0.1:3015");
  assert.equal(origins.clerkOrigin, "http://127.0.0.1:3016");
  assert.equal(origins.apiOrigin, "http://127.0.0.1:8095");
  assert.equal(origins.fixtureOrigin, "http://127.0.0.1:8097");
});
test("an isolated port base gives every owned service a distinct consistent origin", () => {
  for (const base of [1024, 31000, 65532]) {
    const origins = serviceOrigins(String(base));
    const ports = [
      origins.staffPort,
      origins.clerkPort,
      origins.apiPort,
      origins.fixturePort,
    ];
    assert.equal(new Set(ports).size, 4);
    for (const [index, name] of [
      "staff",
      "clerk",
      "api",
      "fixture",
    ].entries()) {
      assert.equal(ports[index], base + index);
      assert.equal(
        new URL(origins[`${name}Origin`]).port,
        String(ports[index]),
      );
    }
  }
});
test("invalid port bases fail before launching or contacting another service", () => {
  for (const base of [
    "0",
    "1023",
    "65533",
    "999999999999999999",
    "31000.5",
    "31000/path",
    " 31000",
    "NaN",
    "-1",
  ])
    assert.throws(() => serviceOrigins(base), /FREIGHT_E2E_PORT_BASE/);
});
test(
  "owned Linux browser keeps Unicode download names under a minimal locale",
  {
    skip: process.platform !== "linux",
  },
  () => {
    const require = createRequire(import.meta.url);
    const previous = { LANG: process.env.LANG, LC_ALL: process.env.LC_ALL };
    try {
      process.env.LANG = "C";
      delete process.env.LC_ALL;
      const config = require("../playwright.config.js");
      assert.equal(config.use.launchOptions.env.LANG, "C.UTF-8");
      assert.equal(config.use.launchOptions.env.LC_ALL, "C.UTF-8");
    } finally {
      for (const [name, value] of Object.entries(previous)) {
        if (value === undefined) delete process.env[name];
        else process.env[name] = value;
      }
    }
  },
);
