const { defineConfig } = require("@playwright/test");
const {
  staffPort,
  clerkPort,
  apiPort,
  fixturePort,
  staffOrigin,
  clerkOrigin,
  apiOrigin,
  fixtureOrigin,
} = require("./tests/service-origins.cjs")();
module.exports = defineConfig({
  testDir: "./tests",
  testMatch: "**/*.spec.js",
  workers: 1,
  timeout: 45000,
  use: {
    baseURL: process.env.FREIGHT_E2E_URL || staffOrigin,
    launchOptions: {
      // Snap Chromium uses the process locale for Unicode download filenames.
      env:
        process.platform === "linux"
          ? { ...process.env, LANG: "C.UTF-8", LC_ALL: "C.UTF-8" }
          : undefined,
      downloadsPath: process.env.FREIGHT_E2E_DOWNLOADS_DIR || undefined,
      executablePath:
        process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH || undefined,
      args: ["--no-sandbox"],
    },
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
  },
  webServer:
    process.env.FREIGHT_E2E_EXTERNAL === "true"
      ? undefined
      : [
          {
            command: "node tests/edi-fixture.mjs",
            env: {
              FREIGHT_E2E_PORT_BASE: process.env.FREIGHT_E2E_PORT_BASE || "",
            },
            url: `${fixtureOrigin}/health`,
          },
          {
            command:
              process.env.FREIGHT_E2E_API_BINARY ||
              "../target/debug/stateset-freight",
            url: `${apiOrigin}/health`,
            env: {
              FREIGHT_DEMO: "true",
              FREIGHT_BIND_ADDR: `127.0.0.1:${apiPort}`,
              DATABASE_URL: process.env.FREIGHT_E2E_DATABASE_URL || "",
              FREIGHT_CLERK_JSON: JSON.stringify({
                issuer: fixtureOrigin,
                secret_key: "sk_test_owned_fixture_only_at_least_32_characters",
                fixture_api_url: fixtureOrigin,
                authorized_parties: [clerkOrigin],
                organizations: {
                  org_carrier: {
                    tenant_id: "demo",
                    counterparty_id: "ca4",
                    roles: { "org:carrier": "carrier" },
                  },
                  org_customer: {
                    tenant_id: "demo",
                    counterparty_id: "c1",
                    roles: { "org:customer": "customer" },
                  },
                  org_staff: {
                    tenant_id: "identity",
                    roles: {
                      "org:operations": "operations",
                      "org:finance": "finance",
                      "org:viewer": "viewer",
                    },
                  },
                  org_other: {
                    tenant_id: "identity_other",
                    roles: { "org:operations": "operations" },
                  },
                  org_sdk_staff: {
                    tenant_id: "sdk-identity",
                    roles: {
                      "org:operations": "operations",
                      "org:viewer": "viewer",
                    },
                  },
                  org_sdk_other: {
                    tenant_id: "sdk-identity-other",
                    roles: { "org:operations": "operations" },
                  },
                  org_sdk_carrier: {
                    tenant_id: "demo",
                    counterparty_id: "ca4",
                    roles: { "org:carrier": "carrier" },
                  },
                },
              }),
              FREIGHT_API_KEYS_JSON: JSON.stringify({
                "freight-test-invoice-maker-credential-at-least-32-chars": {
                  tenant_id: "browser-invoice-review",
                  subject: "browser:invoice-maker",
                  role: "finance",
                },
                "freight-test-invoice-checker-credential-at-least-32-chars": {
                  tenant_id: "browser-invoice-review",
                  subject: "browser:invoice-checker",
                  role: "finance",
                },
                "freight-test-invoice-maker-admin-credential-at-least-32-chars":
                  {
                    tenant_id: "browser-invoice-review",
                    subject: "browser:invoice-maker",
                    role: "admin",
                  },
                "freight-test-carrier-credential-at-least-32-chars": {
                  tenant_id: "demo",
                  subject: "test:carrier",
                  role: "carrier",
                  counterparty_id: "ca4",
                },
                "freight-test-other-carrier-credential-at-least-32-chars": {
                  tenant_id: "demo",
                  subject: "test:other-carrier",
                  role: "carrier",
                  counterparty_id: "ca1",
                },
                "freight-test-customer-credential-at-least-32-chars": {
                  tenant_id: "demo",
                  subject: "test:customer",
                  role: "customer",
                  counterparty_id: "c1",
                },
                "freight-test-other-customer-credential-at-least-32-chars": {
                  tenant_id: "demo",
                  subject: "test:other-customer",
                  role: "customer",
                  counterparty_id: "c3",
                },
                "freight-test-operator-credential-at-least-32-chars": "demo",
                "freight-test-quote-source-credential-at-least-32-chars":
                  "quote-source",
                "freight-test-source-viewer-credential-at-least-32-chars": {
                  tenant_id: "quote-source",
                  subject: "test:source-viewer",
                  role: "viewer",
                },
                "freight-recovery-test-credential-at-least-32-chars":
                  "recovery",
                ...Object.fromEntries(
                  ["viewer", "operations", "finance", "integration"].map(
                    (role) => [
                      `freight-test-${role}-credential-at-least-32-chars`,
                      { tenant_id: "demo", subject: `test:${role}`, role },
                    ],
                  ),
                ),
              }),
              FREIGHT_INTEGRATIONS_JSON: JSON.stringify({
                demo: {
                  sync_url: fixtureOrigin,
                  sync_api_key: "freight-source-sync-fixture",
                  sync_tenant_id: "00000000-0000-4000-8000-000000000901",
                },
                "quote-source": {
                  sync_url: fixtureOrigin,
                  sync_api_key: "freight-source-sync-fixture",
                  sync_tenant_id: "00000000-0000-4000-8000-000000000901",
                },
                recovery: {
                  edi_url: fixtureOrigin,
                  edi_token: "freight-fixture-only",
                  edi_tenant_id: "recovery",
                  callback_keys: {
                    1: "freight-recovery-callback-secret-at-least-32-chars",
                  },
                },
              }),
            },
          },
          {
            command:
              process.env.FREIGHT_E2E_PRODUCTION === "true"
                ? "node scripts/start.mjs"
                : `node node_modules/next/dist/bin/next dev --hostname 127.0.0.1 --port ${staffPort}`,
            url: staffOrigin,
            timeout: 60000,
            env: {
              FREIGHT_DEMO: "false",
              FREIGHT_WEB_AUTH_MODE: "credential",
              FREIGHT_WEB_ORIGIN: staffOrigin,
              FREIGHT_API_URL: apiOrigin,
              FREIGHT_SESSION_SECRET:
                "freight-test-session-secret-at-least-32-characters",
              FREIGHT_COOKIE_SECURE: "false",
              PORT: String(staffPort),
              FREIGHT_WEB_BIND: "127.0.0.1",
            },
          },
          {
            command:
              process.env.FREIGHT_E2E_PRODUCTION === "true"
                ? "node .next/standalone/server.js"
                : `node node_modules/next/dist/bin/next dev --hostname 127.0.0.1 --port ${clerkPort}`,
            url: clerkOrigin,
            timeout: 60000,
            env: {
              FREIGHT_DEMO: "false",
              FREIGHT_WEB_AUTH_MODE: "clerk",
              FREIGHT_SAVE_JOURNAL_KEYS_JSON: JSON.stringify({
                active: "fixture",
                keys: { fixture: Buffer.alloc(32, 7).toString("base64url") },
              }),
              FREIGHT_CLERK_PUBLISHABLE_KEY: `pk_test_${Buffer.from("clerk.fixture.test$").toString("base64url")}`,
              FREIGHT_WEB_ORIGIN: clerkOrigin,
              FREIGHT_API_URL: apiOrigin,
              PORT: String(clerkPort),
              HOSTNAME: "127.0.0.1",
            },
          },
        ],
});
