const { test, expect } = require("@playwright/test");

const BASE = "http://127.0.0.1:4010";
const API = `${BASE}/api/v1`;

// Helper: get a session token via the JSON test-token endpoint.
// This avoids the redirect/cookie issue with /auth/test-login.
async function getSessionToken(request) {
  const resp = await request.post(`${API}/auth/test-token`);
  expect(resp.status()).toBe(200);
  const body = await resp.json();
  expect(body.session_token).toBeTruthy();
  return body.session_token;
}

// Helper: create an API key using session auth
async function createApiKey(request, sessionToken, name) {
  const resp = await request.post(`${API}/auth/keys`, {
    headers: { Cookie: `session=${sessionToken}` },
    data: { name },
  });
  expect(resp.status()).toBe(201);
  const body = await resp.json();
  expect(body.key).toBeTruthy();
  expect(body.key).toMatch(/^boop_/);
  return body.key;
}

test.describe("Public API", () => {
  let apiKey;
  let sessionToken;

  test.beforeAll(async ({ request }) => {
    sessionToken = await getSessionToken(request);
    apiKey = await createApiKey(request, sessionToken, "e2e-test-key");
  });

  function authHeaders() {
    return { Authorization: `Bearer ${apiKey}` };
  }

  test("unauthenticated requests return 401", async ({ request }) => {
    const resp = await request.get(`${API}/bookmarks`);
    expect(resp.status()).toBe(401);
  });

  test("invalid API key returns 401", async ({ request }) => {
    const resp = await request.get(`${API}/bookmarks`, {
      headers: { Authorization: "Bearer boop_invalid_key" },
    });
    expect(resp.status()).toBe(401);
  });

  test("CRUD bookmark lifecycle", async ({ request }) => {
    // Create
    const createResp = await request.post(`${API}/bookmarks`, {
      headers: authHeaders(),
      data: {
        url: "https://example.com/api-test",
        title: "API Test Bookmark",
        description: "Created via API",
        tags: ["api", "test"],
      },
    });
    expect(createResp.status()).toBe(201);
    const bookmark = await createResp.json();
    expect(bookmark.id).toBeTruthy();
    expect(bookmark.url).toBe("https://example.com/api-test");
    expect(bookmark.title).toBe("API Test Bookmark");
    expect(bookmark.tags).toEqual(["api", "test"]);

    // Get
    const getResp = await request.get(`${API}/bookmarks/${bookmark.id}`, {
      headers: authHeaders(),
    });
    expect(getResp.status()).toBe(200);
    const fetched = await getResp.json();
    expect(fetched.id).toBe(bookmark.id);

    // Update
    const updateResp = await request.put(`${API}/bookmarks/${bookmark.id}`, {
      headers: authHeaders(),
      data: {
        title: "Updated Title",
        tags: ["api", "test", "updated"],
      },
    });
    expect(updateResp.status()).toBe(200);
    const updated = await updateResp.json();
    expect(updated.title).toBe("Updated Title");
    expect(updated.tags).toEqual(["api", "test", "updated"]);

    // List
    const listResp = await request.get(`${API}/bookmarks`, {
      headers: authHeaders(),
    });
    expect(listResp.status()).toBe(200);
    const bookmarks = await listResp.json();
    expect(bookmarks.length).toBeGreaterThanOrEqual(1);

    // Search
    const searchResp = await request.get(`${API}/bookmarks?search=Updated`, {
      headers: authHeaders(),
    });
    expect(searchResp.status()).toBe(200);
    const searched = await searchResp.json();
    expect(searched.some((b) => b.id === bookmark.id)).toBe(true);

    // Tags
    const tagsResp = await request.get(`${API}/bookmarks/tags`, {
      headers: authHeaders(),
    });
    expect(tagsResp.status()).toBe(200);
    const tags = await tagsResp.json();
    expect(tags.some((t) => t.name === "api")).toBe(true);

    // Delete
    const deleteResp = await request.delete(`${API}/bookmarks/${bookmark.id}`, {
      headers: authHeaders(),
    });
    expect(deleteResp.status()).toBe(204);

    // Verify deleted
    const verifyResp = await request.get(`${API}/bookmarks/${bookmark.id}`, {
      headers: authHeaders(),
    });
    expect(verifyResp.status()).toBe(404);
  });

  test("list API keys", async ({ request }) => {
    const resp = await request.get(`${API}/auth/keys`, {
      headers: authHeaders(),
    });
    expect(resp.status()).toBe(200);
    const keys = await resp.json();
    expect(keys.length).toBeGreaterThanOrEqual(1);
    expect(keys[0].id).toBeTruthy();
    expect(keys[0].name).toBeTruthy();
    expect(keys[0].created_at).toBeTruthy();
    // Ensure key_hash is not exposed
    expect(keys[0].key_hash).toBeUndefined();
  });

  test("delete API key", async ({ request }) => {
    // Create a temporary key
    const tempKey = await createApiKey(request, sessionToken, "temp-key");

    // List to get ID
    const listResp = await request.get(`${API}/auth/keys`, {
      headers: { Authorization: `Bearer ${tempKey}` },
    });
    const keys = await listResp.json();
    const tempKeyEntry = keys.find((k) => k.name === "temp-key");
    expect(tempKeyEntry).toBeTruthy();

    // Delete it
    const deleteResp = await request.delete(`${API}/auth/keys/${tempKeyEntry.id}`, {
      headers: { Authorization: `Bearer ${tempKey}` },
    });
    expect(deleteResp.status()).toBe(204);

    // Verify the key no longer works
    const verifyResp = await request.get(`${API}/bookmarks`, {
      headers: { Authorization: `Bearer ${tempKey}` },
    });
    expect(verifyResp.status()).toBe(401);
  });
});
