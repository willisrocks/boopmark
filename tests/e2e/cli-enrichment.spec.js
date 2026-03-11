const { test, expect } = require("@playwright/test");
const { execSync } = require("child_process");
const fs = require("fs");
const path = require("path");
const os = require("os");

const BOOP = path.join(process.cwd(), "target", "debug", "boop");

function runBoop(args, apiKey) {
  const tempHome = fs.mkdtempSync(path.join(os.tmpdir(), "boop-e2e-"));
  // dirs::config_dir() on macOS = $HOME/Library/Application Support
  const configDir = path.join(
    tempHome,
    "Library",
    "Application Support",
    "boop"
  );
  fs.mkdirSync(configDir, { recursive: true });
  fs.writeFileSync(
    path.join(configDir, "config.toml"),
    `server_url = "http://127.0.0.1:4010"\napi_key = "${apiKey}"\n`
  );

  const env = { ...process.env, HOME: tempHome };
  return execSync(`${BOOP} ${args}`, {
    env,
    encoding: "utf-8",
    timeout: 30000,
  });
}

async function signIn(page) {
  await page.goto("/");
  await page.getByRole("button", { name: "Sign in for E2E" }).click();
  await expect(page).toHaveURL(/\/bookmarks$/);
}

async function createApiKey(page, name) {
  await page.goto("/settings");
  await page.getByTestId("api-key-name-input").fill(name);
  await page.getByTestId("create-api-key-button").click();
  await expect(page.getByTestId("api-key-created-notice")).toBeVisible();
  return await page.getByTestId("api-key-raw-value").textContent();
}

test.describe("CLI enrichment", () => {
  test.beforeAll(async () => {
    execSync("cargo build -p boop", { stdio: "inherit", timeout: 120000 });
  });

  // Test 8: boop add creates a bookmark and shows output
  test("boop add creates a bookmark and shows output", async ({ page }) => {
    await signIn(page);
    const apiKey = await createApiKey(page, "cli-add-test");

    const output = runBoop(
      'add "https://example.com/cli-test-1"',
      apiKey
    );
    expect(output).toContain("Added:");
    expect(output).toContain("(");
  });

  // Test 9: boop add --description passes description to API
  test("boop add --description includes description", async ({ page }) => {
    await signIn(page);
    const apiKey = await createApiKey(page, "cli-desc-test");

    const output = runBoop(
      'add "https://example.com/cli-desc-test" --description "A test description"',
      apiKey
    );
    expect(output).toContain("Added:");
    expect(output).toContain("A test description");
  });

  // Test 10: boop suggest returns suggestions without saving
  test("boop suggest returns suggestions", async ({ page }) => {
    await signIn(page);
    const apiKey = await createApiKey(page, "cli-suggest-test");

    const output = runBoop('suggest "http://127.0.0.1:4010/"', apiKey);
    expect(output).toBeDefined();
  });

  // Test 11: boop edit with explicit fields updates a bookmark
  test("boop edit with explicit fields updates a bookmark", async ({
    page,
    browser,
  }) => {
    await signIn(page);
    const apiKey = await createApiKey(page, "cli-edit-test");

    // Create a bookmark via API
    const freshContext = await browser.newContext();
    const freshPage = await freshContext.newPage();
    await freshPage.goto(page.url());

    const createResp = await freshPage.evaluate(async (key) => {
      const response = await fetch("/api/v1/bookmarks", {
        method: "POST",
        headers: {
          Authorization: `Bearer ${key}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({ url: "https://example.com/cli-edit-test" }),
      });
      return response.json();
    }, apiKey);

    const bookmarkId = createResp.id;
    await freshContext.close();

    const output = runBoop(
      `edit ${bookmarkId} --title "New Title" --description "New Desc"`,
      apiKey
    );
    expect(output).toContain("Updated: New Title");
  });

  // Test 12: boop edit --suggest updates a bookmark with enrichment
  test("boop edit --suggest updates a bookmark with enrichment", async ({
    page,
    browser,
  }) => {
    await signIn(page);
    const apiKey = await createApiKey(page, "cli-edit-suggest-test");

    // Create a bookmark via API
    const freshContext = await browser.newContext();
    const freshPage = await freshContext.newPage();
    await freshPage.goto(page.url());

    const createResp = await freshPage.evaluate(async (key) => {
      const response = await fetch("/api/v1/bookmarks", {
        method: "POST",
        headers: {
          Authorization: `Bearer ${key}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({ url: "http://127.0.0.1:4010/" }),
      });
      return response.json();
    }, apiKey);

    const bookmarkId = createResp.id;
    await freshContext.close();

    const output = runBoop(`edit ${bookmarkId} --suggest`, apiKey);
    expect(output).toContain("Updated:");
  });

  // Test 13: boop add --suggest creates with enrichment
  test("boop add --suggest creates with enrichment", async ({ page }) => {
    await signIn(page);
    const apiKey = await createApiKey(page, "cli-add-suggest-test");

    const output = runBoop(
      'add "http://127.0.0.1:4010/" --suggest',
      apiKey
    );
    expect(output).toContain("Added:");
  });

  // Clean up API keys created by this spec to avoid polluting other specs
  test.afterAll(async ({ browser }) => {
    const context = await browser.newContext();
    const page = await context.newPage();
    await signIn(page);
    const keys = await page.evaluate(async () => {
      const response = await fetch("/api/v1/auth/keys");
      return response.json();
    });
    for (const key of keys) {
      await page.evaluate(async (id) => {
        await fetch(`/api/v1/auth/keys/${id}`, { method: "DELETE" });
      }, key.id);
    }
    await context.close();
  });
});
