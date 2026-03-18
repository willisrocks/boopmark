// @ts-check
const { test, expect } = require("@playwright/test");
const { execSync } = require("child_process");
const fs = require("fs");
const path = require("path");
const os = require("os");

const BOOP = path.join(process.cwd(), "target", "debug", "boop");

function runBoop(args, apiKey) {
  const tempHome = fs.mkdtempSync(path.join(os.tmpdir(), "boop-fix-images-"));
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
    timeout: 60000,
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
  // The raw key is displayed after creation — grab it from the page
  const rawKeyEl = page.locator("[data-testid='raw-api-key']");
  if (await rawKeyEl.count() > 0) {
    return await rawKeyEl.textContent();
  }
  // Fallback: look for a code/pre element containing the key text
  const keyText = await page.locator("code, .raw-key, [data-raw-key]").first().textContent();
  return keyText?.trim() ?? "";
}

// ─── API tests ─────────────────────────────────────────────────────────────

test.describe("fix-images API", () => {
  test("POST /api/v1/bookmarks/fix-images returns 401 when unauthenticated", async ({
    request,
  }) => {
    // Use bare `request` fixture (no session) to test unauthenticated access
    const response = await request.post(
      "http://127.0.0.1:4010/api/v1/bookmarks/fix-images",
      { headers: { Accept: "text/event-stream" } }
    );
    expect(response.status()).toBe(401);
  });

  test("POST /api/v1/bookmarks/fix-images streams SSE and completes with done:true", async ({
    page,
  }) => {
    await signIn(page);

    // Use page.request so session cookies are sent automatically
    const response = await page.request.post(
      "/api/v1/bookmarks/fix-images",
      { headers: { Accept: "text/event-stream" } }
    );
    expect(response.status()).toBe(200);

    const body = await response.text();
    expect(body).toContain("data:");
    expect(body).toContain('"done"');

    // Parse data lines and verify the last event has done:true
    const lines = body.split("\n").filter((l) => l.startsWith("data: "));
    expect(lines.length).toBeGreaterThan(0);
    const last = JSON.parse(lines[lines.length - 1].slice(6));
    expect(last.done).toBe(true);
    expect(typeof last.fixed).toBe("number");
    expect(typeof last.failed).toBe("number");
    expect(typeof last.checked).toBe("number");
    expect(typeof last.total).toBe("number");
  });

  test("POST /api/v1/bookmarks/fix-images returns 409 on concurrent job", async ({
    page,
  }) => {
    await signIn(page);

    // Start the first request and wait for its response headers (which arrive
    // before the SSE body is drained). This guarantees the job lock is held
    // before the second request is sent, making the 409 check deterministic.
    const statuses = await page.evaluate(async () => {
      const url = "/api/v1/bookmarks/fix-images";
      const opts = { method: "POST", headers: { Accept: "text/event-stream" } };
      // r1 headers arrive while the job is still running (holding the lock)
      const r1 = await fetch(url, opts);
      // send r2 only after r1 headers are received — lock is held
      const r2 = await fetch(url, opts);
      return [r1.status, r2.status];
    });

    const statusSet = new Set(statuses);
    expect(statusSet.has(200)).toBe(true);
    expect(statusSet.has(409)).toBe(true);
  });
});

// ─── Web UI tests ───────────────────────────────────────────────────────────

test.describe("fix-images settings UI", () => {
  test("Settings page has Image Repair section", async ({ page }) => {
    await signIn(page);
    await page.goto("/settings");
    await expect(page.getByText("Image Repair")).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Fix Missing Images" })
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Fix Missing Images" })
    ).toBeEnabled();
    // Progress section starts hidden
    await expect(page.locator("#fix-images-progress")).toHaveClass(/hidden/);
  });

  test("Clicking Fix Missing Images shows progress and completes", async ({
    page,
  }) => {
    await signIn(page);
    await page.goto("/settings");

    const btn = page.getByRole("button", { name: "Fix Missing Images" });
    const progressSection = page.locator("#fix-images-progress");
    const label = page.locator("#fix-images-label");

    await expect(progressSection).toHaveClass(/hidden/);
    await btn.click();
    await expect(progressSection).not.toHaveClass(/hidden/);

    // Wait for completion message (timeout 30s)
    await expect(label).toContainText("Done.", { timeout: 30000 });
    await expect(btn).toBeEnabled();
  });
});

// ─── CLI tests ─────────────────────────────────────────────────────────────

test.describe("fix-images CLI", () => {
  test("boop images fix --help shows expected output", async () => {
    const output = execSync("cargo run -p boop -- images fix --help", {
      encoding: "utf8",
      cwd: process.cwd(),
    });
    expect(output).toContain("fix");
    expect(output.toLowerCase()).toContain("image");
  });

  test("boop images fix completes with Done output", async ({ page }) => {
    await signIn(page);

    // Create an API key via the settings UI
    await page.goto("/settings");
    await page.getByTestId("api-key-name-input").fill("fix-images-e2e");
    await page.getByTestId("create-api-key-button").click();

    // The raw key is shown in the created notice — use the stable testid
    const rawKeyEl = page.getByTestId("api-key-raw-value");
    await expect(rawKeyEl).toBeVisible();
    const apiKey = (await rawKeyEl.textContent()) ?? "";
    expect(apiKey).toMatch(/^boop_/);

    const output = runBoop("images fix", apiKey);
    expect(output).toContain("Done.");
    expect(output.toLowerCase()).toContain("fixed");
  });
});
