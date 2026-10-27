const { defineConfig } = require("@playwright/test");
const base = require("./playwright.config.js");
module.exports = defineConfig({
  ...base,
  testMatch: "**/itinerary-refresh.spec.js",
  webServer: undefined,
  outputDir: "test-results-offline",
  use: { ...base.use, baseURL: undefined, offline: true },
});
