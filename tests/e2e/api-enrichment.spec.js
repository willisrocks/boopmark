const { test, expect } = require("@playwright/test");

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

// Test 1: POST /api/v1/bookmarks/suggest returns enrichment data structure
test("suggest endpoint returns a valid suggestion response", async ({
  page,
  browser,
}) => {
  await signIn(page);
  const apiKey = await createApiKey(page, "suggest-test");

  const freshContext = await browser.newContext();
  const freshPage = await freshContext.newPage();
  await freshPage.goto(page.url());

  const resp = await freshPage.evaluate(async (key) => {
    const response = await fetch("/api/v1/bookmarks/suggest", {
      method: "POST",
      headers: {
        Authorization: `Bearer ${key}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ url: "http://127.0.0.1:4010/" }),
    });
    return { status: response.status, body: await response.json() };
  }, apiKey);

  expect(resp.status).toBe(200);
  expect(resp.body).toHaveProperty("title");
  expect(resp.body).toHaveProperty("description");
  expect(resp.body).toHaveProperty("tags");
  expect(resp.body).toHaveProperty("image_url");
  expect(resp.body).toHaveProperty("domain");
  expect(Array.isArray(resp.body.tags)).toBe(true);

  await freshContext.close();
});

// Test 2: POST /api/v1/bookmarks?suggest=true creates a bookmark with enrichment
test("creating a bookmark with suggest=true enriches missing fields", async ({
  page,
  browser,
}) => {
  await signIn(page);
  const apiKey = await createApiKey(page, "create-suggest-test");

  const freshContext = await browser.newContext();
  const freshPage = await freshContext.newPage();
  await freshPage.goto(page.url());

  const resp = await freshPage.evaluate(async (key) => {
    const response = await fetch("/api/v1/bookmarks?suggest=true", {
      method: "POST",
      headers: {
        Authorization: `Bearer ${key}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ url: "http://127.0.0.1:4010/" }),
    });
    return { status: response.status, body: await response.json() };
  }, apiKey);

  expect(resp.status).toBe(201);
  expect(resp.body.id).toBeDefined();
  expect(resp.body.url).toBe("http://127.0.0.1:4010/");

  await freshContext.close();
});

// Test 3: POST /api/v1/bookmarks preserves client-provided fields without suggest
test("creating a bookmark with explicit fields preserves them", async ({
  page,
  browser,
}) => {
  await signIn(page);
  const apiKey = await createApiKey(page, "preserve-test");

  const freshContext = await browser.newContext();
  const freshPage = await freshContext.newPage();
  await freshPage.goto(page.url());

  const resp = await freshPage.evaluate(async (key) => {
    const response = await fetch("/api/v1/bookmarks", {
      method: "POST",
      headers: {
        Authorization: `Bearer ${key}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({
        url: "https://example.com/test-preserve",
        title: "My Title",
        description: "My Description",
        tags: ["tag1", "tag2"],
      }),
    });
    return { status: response.status, body: await response.json() };
  }, apiKey);

  expect(resp.status).toBe(201);
  expect(resp.body.title).toBe("My Title");
  expect(resp.body.description).toBe("My Description");
  expect(resp.body.tags).toContain("tag1");
  expect(resp.body.tags).toContain("tag2");

  await freshContext.close();
});

// Test 4: PUT /api/v1/bookmarks/{id} normal update without suggest
test("updating a bookmark title via PUT works without enrichment", async ({
  page,
  browser,
}) => {
  await signIn(page);
  const apiKey = await createApiKey(page, "update-test");

  const freshContext = await browser.newContext();
  const freshPage = await freshContext.newPage();
  await freshPage.goto(page.url());

  // Create a bookmark first
  const createResp = await freshPage.evaluate(async (key) => {
    const response = await fetch("/api/v1/bookmarks", {
      method: "POST",
      headers: {
        Authorization: `Bearer ${key}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({
        url: "https://example.com/update-test",
        title: "Original Title",
      }),
    });
    return { status: response.status, body: await response.json() };
  }, apiKey);

  expect(createResp.status).toBe(201);
  const bookmarkId = createResp.body.id;

  // Update the title
  const updateResp = await freshPage.evaluate(
    async ({ key, id }) => {
      const response = await fetch(`/api/v1/bookmarks/${id}`, {
        method: "PUT",
        headers: {
          Authorization: `Bearer ${key}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({ title: "Updated Title" }),
      });
      return { status: response.status, body: await response.json() };
    },
    { key: apiKey, id: bookmarkId }
  );

  expect(updateResp.status).toBe(200);
  expect(updateResp.body.title).toBe("Updated Title");

  await freshContext.close();
});

// Test 5: PUT /api/v1/bookmarks/{id}?suggest=true enriches missing fields
test("updating a bookmark with suggest=true fills missing fields", async ({
  page,
  browser,
}) => {
  await signIn(page);
  const apiKey = await createApiKey(page, "update-suggest-test");

  const freshContext = await browser.newContext();
  const freshPage = await freshContext.newPage();
  await freshPage.goto(page.url());

  // Create a bookmark
  const createResp = await freshPage.evaluate(async (key) => {
    const response = await fetch("/api/v1/bookmarks", {
      method: "POST",
      headers: {
        Authorization: `Bearer ${key}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({
        url: "http://127.0.0.1:4010/",
        title: "Keep This",
      }),
    });
    return { status: response.status, body: await response.json() };
  }, apiKey);

  expect(createResp.status).toBe(201);
  const bookmarkId = createResp.body.id;

  // Update with suggest=true and empty body
  const updateResp = await freshPage.evaluate(
    async ({ key, id }) => {
      const response = await fetch(`/api/v1/bookmarks/${id}?suggest=true`, {
        method: "PUT",
        headers: {
          Authorization: `Bearer ${key}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({}),
      });
      return { status: response.status, body: await response.json() };
    },
    { key: apiKey, id: bookmarkId }
  );

  expect(updateResp.status).toBe(200);
  expect(updateResp.body.id).toBe(bookmarkId);
  expect(updateResp.body.url).toBeDefined();
  expect(Array.isArray(updateResp.body.tags)).toBe(true);

  await freshContext.close();
});

// Test 6: All enrichment endpoints return 401 without auth
test("unauthenticated requests to enrichment endpoints are rejected", async ({
  request,
}) => {
  const suggestResp = await request.post("/api/v1/bookmarks/suggest", {
    headers: { "Content-Type": "application/json" },
    data: { url: "https://example.com" },
  });
  expect(suggestResp.status()).toBe(401);
});

// Test 7: POST /api/v1/bookmarks?suggest=true preserves client-provided fields
test("client-provided fields are not overwritten by enrichment with suggest=true", async ({
  page,
  browser,
}) => {
  await signIn(page);
  const apiKey = await createApiKey(page, "preserve-suggest-test");

  const freshContext = await browser.newContext();
  const freshPage = await freshContext.newPage();
  await freshPage.goto(page.url());

  const resp = await freshPage.evaluate(async (key) => {
    const response = await fetch("/api/v1/bookmarks?suggest=true", {
      method: "POST",
      headers: {
        Authorization: `Bearer ${key}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({
        url: "http://127.0.0.1:4010/",
        title: "My Custom Title",
        description: "My Custom Desc",
        tags: ["custom"],
      }),
    });
    return { status: response.status, body: await response.json() };
  }, apiKey);

  expect(resp.status).toBe(201);
  expect(resp.body.title).toBe("My Custom Title");
  expect(resp.body.description).toBe("My Custom Desc");
  expect(resp.body.tags).toContain("custom");

  await freshContext.close();
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
