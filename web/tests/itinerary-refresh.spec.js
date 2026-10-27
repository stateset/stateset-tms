const { test, expect } = require("@playwright/test");
const { readFile } = require("node:fs/promises");
const path = require("node:path");
const { transform } = require("next/dist/build/swc");

let modules;
test.beforeAll(async () => {
  modules = await Promise.all(
    ["../lib/modal-isolation.mjs", "Modal", "ItineraryExecution"].map(
      async (name) => {
        const filename = name.endsWith(".mjs")
          ? path.join(__dirname, name)
          : path.join(__dirname, "../components", `${name}.js`);
        const result = await transform(await readFile(filename, "utf8"), {
          filename,
          jsc: {
            parser: { syntax: "ecmascript", jsx: true },
            transform: { react: { runtime: "classic" } },
          },
          module: { type: "commonjs" },
        });
        return { name, code: result.code };
      },
    ),
  );
});

async function mount(page, uncertain = false) {
  // The real components run without a web server or network. Only their
  // identity adapter and refreshed source props are controlled by this fixture.
  await page.setContent('<main id="fixture"></main>');
  for (const [packageName, file] of [
    ["react", "react.development.js"],
    ["react-dom", "react-dom.development.js"],
  ]) {
    await page.addScriptTag({
      content: await readFile(
        path.join(
          path.dirname(require.resolve(`${packageName}/package.json`)),
          "umd",
          file,
        ),
        "utf8",
      ),
    });
  }
  await page.evaluate(
    ({ modules, uncertain }) => {
      window.mutations = [];
      window.refreshAttempts = 0;
      let requestId = 0;
      // about:blank may lack secure-context UUID support. Distinct fixture IDs
      // preserve the ability to detect a mistakenly regenerated retry key.
      if (!crypto.randomUUID)
        crypto.randomUUID = () =>
          `00000000-0000-4000-8000-${String(++requestId).padStart(12, "0")}`;
      window.fixtureIdentity = {
        fetch: async (url, options) => {
          window.mutations.push({
            url,
            key: options.headers["Idempotency-Key"],
            body: JSON.parse(options.body),
          });
          if (uncertain && window.mutations.length === 1)
            throw new Error("Owned uncertain save response");
          return {
            ok: true,
            json: async () => ({ data: { id: "owned-service" } }),
          };
        },
      };
      const imports = {
        react: React,
        "lucide-react": {
          X: () => React.createElement("span", { "aria-hidden": true }),
        },
        "./FreightIdentity": {
          useFreightIdentity: () => window.fixtureIdentity,
        },
      };
      for (const { name, code } of modules) {
        const module = { exports: {} };
        new Function("require", "module", "exports", code)(
          (name) => {
            if (!imports[name])
              throw new Error(`Unexpected fixture dependency: ${name}`);
            return imports[name];
          },
          module,
          module.exports,
        );
        imports[name.startsWith("../") ? name : `./${name}`] = module.exports;
      }
      const original = {
        id: "owned-offer",
        is_current: true,
        status: "in_transit",
        load_version: 5,
        reporting: {
          tracking_kinds: ["location"],
          itinerary: {
            requires_native_handoff: true,
            stops: [
              { id: "pickup", completed: true },
              {
                id: "owned-stop",
                sequence: 2,
                kind: "delivery",
                completed: false,
                stop: {
                  facility: "Owned receiver",
                  city: "Tacoma",
                  state: "WA",
                  date: "2026-10-18",
                },
              },
              { id: "delivery", completed: false },
            ],
          },
        },
      };
      function Fixture() {
        const [offer, setOffer] = React.useState(original);
        const refresh = () => {
          window.refreshAttempts++;
          return new Promise((resolve) => {
            window.failRefresh = () => resolve(false);
            window.finishRefresh = () => {
              const fresh = JSON.parse(JSON.stringify(original));
              fresh.load_version++;
              fresh.reporting.tracking_kinds.push("delivery");
              fresh.reporting.itinerary.stops[1].completed = true;
              fresh.reporting.itinerary.stops[1].service = {
                observed_at: "2026-10-18T15:00:00Z",
              };
              setOffer(fresh);
              resolve(true);
            };
          });
        };
        return React.createElement(
          React.Fragment,
          null,
          React.createElement(
            "p",
            { role: "status" },
            offer.reporting.tracking_kinds.includes("delivery")
              ? "Final delivery available"
              : "Intermediate service missing",
          ),
          React.createElement(
            imports["./ItineraryExecution"].ItineraryExecution,
            { offer, onChanged: refresh },
          ),
        );
      }
      ReactDOM.createRoot(document.getElementById("fixture")).render(
        React.createElement(Fixture),
      );
    },
    { modules, uncertain },
  );
  await page
    .getByRole("button", { name: "Confirm stop 2 service", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Actual service time (UTC)").fill("2026-10-18T15:00");
  await dialog.getByLabel("Actual stop contact").fill("Actual receiver lead");
  await dialog
    .getByLabel("Stop service reference")
    .fill("Owned original service receipt");
  await dialog
    .getByLabel("Stop service findings")
    .fill("Actual unloading verified");
  await dialog.getByRole("checkbox").check();
  return dialog;
}

test("a confirmed intermediate service keeps its dialog held until the refreshed route is available", async ({
  page,
}) => {
  const dialog = await mount(page);
  await dialog
    .getByRole("button", { name: "Save stop confirmation", exact: true })
    .click();
  await expect.poll(() => page.evaluate(() => window.refreshAttempts)).toBe(1);
  await expect(dialog).toBeVisible();
  await expect(dialog.getByLabel("Actual stop contact")).toBeDisabled();
  await page.evaluate(() => window.finishRefresh());
  await expect(dialog).toHaveCount(0);
  await expect(page.getByRole("status")).toHaveText("Final delivery available");
  expect(await page.evaluate(() => window.mutations.length)).toBe(1);
});

test("a confirmed service with a failed refresh retries the read without resubmitting evidence", async ({
  page,
}) => {
  const dialog = await mount(page);
  await dialog
    .getByRole("button", { name: "Save stop confirmation", exact: true })
    .click();
  await expect.poll(() => page.evaluate(() => window.refreshAttempts)).toBe(1);
  await page.evaluate(() => window.failRefresh());
  await expect(dialog.getByRole("alert")).toContainText(
    "Stop confirmation saved",
  );
  await expect(dialog.getByLabel("Actual stop contact")).toBeDisabled();
  await dialog
    .getByRole("button", { name: "Refresh saved confirmation", exact: true })
    .click();
  await expect.poll(() => page.evaluate(() => window.refreshAttempts)).toBe(2);
  expect(await page.evaluate(() => window.mutations.length)).toBe(1);
  await page.evaluate(() => window.finishRefresh());
  await expect(dialog).toHaveCount(0);
  await expect(page.getByRole("status")).toHaveText("Final delivery available");
});

test("an uncertain intermediate save keeps the original body and request key on retry", async ({
  page,
}) => {
  const dialog = await mount(page, true);
  await dialog
    .getByRole("button", { name: "Save stop confirmation", exact: true })
    .click();
  await expect(dialog.getByRole("alert")).toContainText(
    "Owned uncertain save response",
  );
  await expect(dialog.getByLabel("Actual stop contact")).toBeEnabled();
  await dialog
    .getByRole("button", { name: "Save stop confirmation", exact: true })
    .click();
  await expect.poll(() => page.evaluate(() => window.refreshAttempts)).toBe(1);
  const calls = await page.evaluate(() => window.mutations);
  expect(calls).toHaveLength(2);
  expect(calls[1]).toEqual(calls[0]);
  await page.evaluate(() => window.finishRefresh());
  await expect(dialog).toHaveCount(0);
});
