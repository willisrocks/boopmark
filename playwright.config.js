const { defineConfig } = require("@playwright/test");

module.exports = defineConfig({
  testDir: "./tests/e2e",
  fullyParallel: false,
  workers: 1,
  timeout: 60_000,
  expect: { timeout: 10_000 },
  use: {
    baseURL: "http://127.0.0.1:4010",
    trace: "on-first-retry",
    screenshot: "only-on-failure",
  },
  webServer: {
    command: "bash ./scripts/e2e/start-server.sh",
    url: "http://127.0.0.1:4010/health",
    reuseExistingServer: false,
    timeout: 120_000,
  },
});
