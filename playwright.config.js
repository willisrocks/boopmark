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
  webServer: [
    {
      // The E2E server sends every LLM request here (no paid calls).
      command: "node ./scripts/e2e/fake-llm.mjs",
      url: "http://127.0.0.1:4011/health",
      reuseExistingServer: false,
      timeout: 30_000,
    },
    {
      command: "bash ./scripts/e2e/start-server.sh",
      url: "http://127.0.0.1:4010/health",
      reuseExistingServer: false,
      timeout: 300_000,
    },
  ],
});
