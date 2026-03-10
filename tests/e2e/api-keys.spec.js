const { test, expect } = require("@playwright/test");

async function signIn(page) {
  await page.goto("/");
  await page.getByRole("button", { name: "Sign in for E2E" }).click();
  await expect(page).toHaveURL(/\/bookmarks$/);
}

async function deleteAllApiKeys(page) {
  await page.goto("/settings");
  // Delete any existing keys one at a time
  while ((await page.getByTestId("delete-api-key").count()) > 0) {
    await page.getByTestId("delete-api-key").first().click();
    // Wait for HTMX swap to complete
    await page.waitForResponse((resp) =>
      resp.url().includes("/settings/api-keys/")
    );
  }
}

// Test 5 & 6: Settings page shows API Keys section with empty state
test("settings page shows API Keys section with empty state", async ({
  page,
}) => {
  await signIn(page);
  await deleteAllApiKeys(page);

  await expect(page.getByRole("heading", { name: "API Keys" })).toBeVisible();
  await expect(
    page.getByText("Create keys to use the Boopmark API and CLI.")
  ).toBeVisible();
  await expect(page.getByTestId("create-api-key-form")).toBeVisible();
  await expect(page.getByTestId("api-key-name-input")).toBeVisible();
  await expect(page.getByTestId("create-api-key-button")).toBeVisible();
  await expect(page.getByTestId("no-api-keys")).toBeVisible();
  await expect(page.getByTestId("api-key-row")).toHaveCount(0);
});

// Test 1: Full API key lifecycle
test("full API key lifecycle: create, view, persist, delete", async ({
  page,
}) => {
  await signIn(page);
  await deleteAllApiKeys(page);

  // Create a key
  await page.getByTestId("api-key-name-input").fill("my-cli-key");
  await page.getByTestId("create-api-key-button").click();

  // Verify one-time notice
  await expect(page.getByTestId("api-key-created-notice")).toBeVisible();
  await expect(
    page.getByText("Copy it now — it won't be shown again.")
  ).toBeVisible();

  const rawKey = await page.getByTestId("api-key-raw-value").textContent();
  expect(rawKey).toMatch(/^boop_/);

  // Verify key appears in list
  await expect(page.getByTestId("api-key-row")).toHaveCount(1);
  await expect(page.getByTestId("api-key-name").first()).toHaveText(
    "my-cli-key"
  );

  // Reload to verify persistence and notice disappears
  await page.goto("/settings");
  await expect(page.getByTestId("api-key-row")).toHaveCount(1);
  await expect(page.getByTestId("api-key-name").first()).toHaveText(
    "my-cli-key"
  );
  await expect(page.getByTestId("api-key-created-notice")).toHaveCount(0);

  // Delete the key
  await page.getByTestId("delete-api-key").first().click();
  await expect(page.getByTestId("api-key-row")).toHaveCount(0);
  await expect(page.getByTestId("no-api-keys")).toBeVisible();
});

// Test 2: Created API key authenticates against the REST API
test("created API key works for REST API auth", async ({ page }) => {
  await signIn(page);
  await deleteAllApiKeys(page);

  // Create a key and capture the raw value
  await page.getByTestId("api-key-name-input").fill("api-test-key");
  await page.getByTestId("create-api-key-button").click();
  await expect(page.getByTestId("api-key-created-notice")).toBeVisible();

  const rawKey = await page.getByTestId("api-key-raw-value").textContent();

  // Use the key to call the bookmarks API
  const status = await page.evaluate(async (key) => {
    const response = await fetch("/api/v1/bookmarks", {
      headers: { Authorization: `Bearer ${key}` },
    });
    return response.status;
  }, rawKey);

  expect(status).toBe(200);
});

// Test 3: GET /api/v1/auth/keys returns keys without raw key or hash
test("REST API lists API keys with metadata only", async ({ page }) => {
  await signIn(page);
  await deleteAllApiKeys(page);

  // Create a key via UI
  await page.getByTestId("api-key-name-input").fill("rest-list-key");
  await page.getByTestId("create-api-key-button").click();
  await expect(page.getByTestId("api-key-created-notice")).toBeVisible();

  // Call the REST API list endpoint
  const result = await page.evaluate(async () => {
    const response = await fetch("/api/v1/auth/keys");
    return { status: response.status, body: await response.json() };
  });

  expect(result.status).toBe(200);
  expect(Array.isArray(result.body)).toBe(true);
  expect(result.body.length).toBeGreaterThanOrEqual(1);

  const key = result.body.find((k) => k.name === "rest-list-key");
  expect(key).toBeDefined();
  expect(key.id).toBeDefined();
  expect(key.name).toBe("rest-list-key");
  expect(key.created_at).toBeDefined();

  // Ensure no secrets are exposed
  expect(key.key_hash).toBeUndefined();
  expect(key.key).toBeUndefined();
  expect(key.raw_key).toBeUndefined();
  expect(key.user_id).toBeUndefined();
});

// Test 4: DELETE /api/v1/auth/keys/{id} removes a key
test("REST API deletes an API key by ID", async ({ page }) => {
  await signIn(page);
  await deleteAllApiKeys(page);

  // Create a key via UI
  await page.getByTestId("api-key-name-input").fill("delete-me");
  await page.getByTestId("create-api-key-button").click();
  await expect(page.getByTestId("api-key-created-notice")).toBeVisible();

  // Get the key ID via REST API
  const listResult = await page.evaluate(async () => {
    const response = await fetch("/api/v1/auth/keys");
    return response.json();
  });

  const keyToDelete = listResult.find((k) => k.name === "delete-me");
  expect(keyToDelete).toBeDefined();

  // Delete via REST API
  const deleteStatus = await page.evaluate(async (id) => {
    const response = await fetch(`/api/v1/auth/keys/${id}`, {
      method: "DELETE",
    });
    return response.status;
  }, keyToDelete.id);

  expect(deleteStatus).toBe(204);

  // Verify it's gone
  const listAfter = await page.evaluate(async () => {
    const response = await fetch("/api/v1/auth/keys");
    return response.json();
  });

  expect(listAfter.find((k) => k.name === "delete-me")).toBeUndefined();
});

// Test 7: Multiple API keys can coexist and be individually deleted
test("multiple keys coexist and can be individually deleted", async ({
  page,
}) => {
  await signIn(page);
  await deleteAllApiKeys(page);

  // Create key-alpha
  await page.getByTestId("api-key-name-input").fill("key-alpha");
  await page.getByTestId("create-api-key-button").click();
  await expect(page.getByTestId("api-key-created-notice")).toBeVisible();

  // Create key-beta
  await page.getByTestId("api-key-name-input").fill("key-beta");
  await page.getByTestId("create-api-key-button").click();
  await expect(page.getByTestId("api-key-row")).toHaveCount(2);

  // Verify both names are present
  const names = await page.getByTestId("api-key-name").allTextContents();
  expect(names).toContain("key-alpha");
  expect(names).toContain("key-beta");

  // Delete the first key
  await page.getByTestId("delete-api-key").first().click();
  await expect(page.getByTestId("api-key-row")).toHaveCount(1);

  // Verify the remaining key is one of the two
  const remainingName = await page.getByTestId("api-key-name").first().textContent();
  expect(["key-alpha", "key-beta"]).toContain(remainingName);
});

// Test 8: Deleted API key can no longer authenticate
test("deleted API key is rejected by REST API", async ({ page, browser }) => {
  await signIn(page);
  await deleteAllApiKeys(page);

  // Create a key
  await page.getByTestId("api-key-name-input").fill("ephemeral-key");
  await page.getByTestId("create-api-key-button").click();
  await expect(page.getByTestId("api-key-created-notice")).toBeVisible();

  const rawKey = await page.getByTestId("api-key-raw-value").textContent();

  // Verify it works using a fresh context (no session cookie, bearer only)
  const freshContext = await browser.newContext();
  const freshPage = await freshContext.newPage();
  await freshPage.goto(page.url());

  const statusBefore = await freshPage.evaluate(async (key) => {
    const response = await fetch("/api/v1/bookmarks", {
      headers: { Authorization: `Bearer ${key}` },
    });
    return response.status;
  }, rawKey);
  expect(statusBefore).toBe(200);

  // Delete via UI (on the signed-in page)
  await page.getByTestId("delete-api-key").first().click();
  await expect(page.getByTestId("api-key-row")).toHaveCount(0);

  // Verify it's rejected using the fresh context (no session cookie)
  const statusAfter = await freshPage.evaluate(async (key) => {
    const response = await fetch("/api/v1/bookmarks", {
      headers: { Authorization: `Bearer ${key}` },
    });
    return response.status;
  }, rawKey);
  expect(statusAfter).toBe(401);

  await freshContext.close();
});

// Tests 10-13: Unauthenticated requests are rejected
test("unauthenticated requests to API key endpoints return 401", async ({
  request,
}) => {
  // Test 10: POST /settings/api-keys
  const createResponse = await request.post("/settings/api-keys", {
    form: { name: "hacker-key" },
  });
  expect(createResponse.status()).toBe(401);

  // Test 11: DELETE /settings/api-keys/{id}
  const deleteHtmxResponse = await request.delete(
    "/settings/api-keys/00000000-0000-0000-0000-000000000000"
  );
  expect(deleteHtmxResponse.status()).toBe(401);

  // Test 12: GET /api/v1/auth/keys
  const listResponse = await request.get("/api/v1/auth/keys");
  expect(listResponse.status()).toBe(401);

  // Test 13: DELETE /api/v1/auth/keys/{id}
  const deleteRestResponse = await request.delete(
    "/api/v1/auth/keys/00000000-0000-0000-0000-000000000000"
  );
  expect(deleteRestResponse.status()).toBe(401);
});
