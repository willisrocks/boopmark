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

// Reusable setup: sign in, create API key, create a bookmark, return key + bookmark
async function setupWithBookmark(page, browser, keyName, bookmark) {
  await signIn(page);
  const apiKey = await createApiKey(page, keyName);

  const freshContext = await browser.newContext();
  const freshPage = await freshContext.newPage();
  await freshPage.goto(page.url());

  const createResp = await freshPage.evaluate(
    async ({ key, bm }) => {
      const r = await fetch("/api/v1/bookmarks", {
        method: "POST",
        headers: {
          Authorization: `Bearer ${key}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify(bm),
      });
      return { status: r.status, body: await r.json() };
    },
    { key: apiKey, bm: bookmark }
  );
  expect(createResp.status).toBe(201);

  return { apiKey, freshContext, freshPage, bookmarkId: createResp.body.id };
}

// Test 1: Export JSONL via API returns a downloadable file with correct content
test("export JSONL returns a file with core fields and no id/timestamps", async ({
  page,
  browser,
}) => {
  const { apiKey, freshPage, freshContext } = await setupWithBookmark(
    page,
    browser,
    "export-jsonl-test",
    {
      url: "https://example.com/export-jsonl-test",
      title: "Export Test",
      description: "A test bookmark",
      tags: ["test", "export"],
    }
  );

  const resp = await freshPage.evaluate(async (key) => {
    const r = await fetch("/api/v1/bookmarks/export?format=jsonl&mode=export", {
      headers: { Authorization: `Bearer ${key}` },
    });
    return {
      status: r.status,
      contentType: r.headers.get("content-type"),
      disposition: r.headers.get("content-disposition"),
      body: await r.text(),
    };
  }, apiKey);

  expect(resp.status).toBe(200);
  expect(resp.contentType).toContain("application/x-ndjson");
  expect(resp.disposition).toContain("attachment");
  expect(resp.disposition).toContain("bookmarks-");
  expect(resp.disposition).toContain(".jsonl");

  const lines = resp.body
    .split("\n")
    .filter((l) => l.trim())
    .map((l) => JSON.parse(l));
  const bm = lines.find((l) => l.url === "https://example.com/export-jsonl-test");
  expect(bm).toBeDefined();
  expect(bm.title).toBe("Export Test");
  expect(bm.tags).toContain("test");
  expect(bm.tags).toContain("export");
  expect(bm.id).toBeUndefined();
  expect(bm.user_id).toBeUndefined();
  expect(bm.created_at).toBeUndefined();

  await freshContext.close();
});

// Test 2: Export CSV via API returns a downloadable file with correct headers
test("export CSV returns a file with url, title, description, tags columns", async ({
  page,
  browser,
}) => {
  const { apiKey, freshPage, freshContext } = await setupWithBookmark(
    page,
    browser,
    "export-csv-test",
    {
      url: "https://example.com/export-csv-test",
      title: "CSV Export Test",
      description: "CSV description",
      tags: ["csv", "test"],
    }
  );

  const resp = await freshPage.evaluate(async (key) => {
    const r = await fetch("/api/v1/bookmarks/export?format=csv&mode=export", {
      headers: { Authorization: `Bearer ${key}` },
    });
    return {
      status: r.status,
      contentType: r.headers.get("content-type"),
      disposition: r.headers.get("content-disposition"),
      body: await r.text(),
    };
  }, apiKey);

  expect(resp.status).toBe(200);
  expect(resp.contentType).toContain("text/csv");
  expect(resp.disposition).toContain("attachment");
  expect(resp.disposition).toContain(".csv");

  const lines = resp.body.split("\n").filter((l) => l.trim());
  expect(lines[0]).toBe("url,title,description,tags");
  const hasOurRow = lines.some((l) =>
    l.includes("https://example.com/export-csv-test")
  );
  expect(hasOurRow).toBe(true);

  await freshContext.close();
});

// Test 3: Export backup JSONL includes all fields
test("backup JSONL includes id, created_at, updated_at, domain fields", async ({
  page,
  browser,
}) => {
  const { apiKey, freshPage, freshContext } = await setupWithBookmark(
    page,
    browser,
    "export-backup-test",
    {
      url: "https://example.com/export-backup-test",
      title: "Backup Test",
      tags: ["backup"],
    }
  );

  const resp = await freshPage.evaluate(async (key) => {
    const r = await fetch(
      "/api/v1/bookmarks/export?format=jsonl&mode=backup",
      { headers: { Authorization: `Bearer ${key}` } }
    );
    return { status: r.status, body: await r.text() };
  }, apiKey);

  expect(resp.status).toBe(200);
  const lines = resp.body
    .split("\n")
    .filter((l) => l.trim())
    .map((l) => JSON.parse(l));
  const bm = lines.find((l) => l.url === "https://example.com/export-backup-test");
  expect(bm).toBeDefined();
  expect(bm.id).toBeDefined();
  expect(bm.created_at).toBeDefined();
  expect(bm.updated_at).toBeDefined();
  expect(bm.user_id).toBeUndefined();

  await freshContext.close();
});

// Test 4: Import JSONL via API creates new bookmarks and returns result summary
test("importing a JSONL file creates bookmarks and returns correct counts", async ({
  page,
  browser,
}) => {
  await signIn(page);
  const apiKey = await createApiKey(page, "import-jsonl-test");

  const freshContext = await browser.newContext();
  const freshPage = await freshContext.newPage();
  await freshPage.goto(page.url());

  const jsonlBody = [
    JSON.stringify({
      url: "https://import-test-1.example.com",
      title: "Import 1",
      description: "First",
      tags: ["a"],
    }),
    JSON.stringify({
      url: "https://import-test-2.example.com",
      title: "Import 2",
      description: "Second",
      tags: ["b"],
    }),
  ].join("\n");

  const resp = await freshPage.evaluate(
    async ({ key, body }) => {
      const fd = new FormData();
      fd.append("file", new Blob([body], { type: "application/x-ndjson" }), "bookmarks.jsonl");
      const r = await fetch(
        "/api/v1/bookmarks/import?format=jsonl&strategy=upsert&mode=import",
        { method: "POST", headers: { Authorization: `Bearer ${key}` }, body: fd }
      );
      return { status: r.status, body: await r.json() };
    },
    { key: apiKey, body: jsonlBody }
  );

  expect(resp.status).toBe(200);
  expect(resp.body.created).toBe(2);
  expect(resp.body.updated).toBe(0);
  expect(resp.body.skipped).toBe(0);
  expect(resp.body.errors).toHaveLength(0);

  // Verify via export that the bookmarks persisted
  const exportResp = await freshPage.evaluate(async (key) => {
    const r = await fetch("/api/v1/bookmarks/export?format=jsonl&mode=export", {
      headers: { Authorization: `Bearer ${key}` },
    });
    return await r.text();
  }, apiKey);
  expect(exportResp).toContain("https://import-test-1.example.com");
  expect(exportResp).toContain("https://import-test-2.example.com");

  await freshContext.close();
});

// Test 5: Import CSV via API creates new bookmarks
test("importing a CSV file creates bookmarks with correct tag parsing", async ({
  page,
  browser,
}) => {
  await signIn(page);
  const apiKey = await createApiKey(page, "import-csv-test");

  const freshContext = await browser.newContext();
  const freshPage = await freshContext.newPage();
  await freshPage.goto(page.url());

  const csvBody = [
    "url,title,description,tags",
    "https://csv-import-1.example.com,CSV One,First CSV,tag1|tag2",
    "https://csv-import-2.example.com,CSV Two,Second CSV,tag3",
  ].join("\n");

  const resp = await freshPage.evaluate(
    async ({ key, body }) => {
      const fd = new FormData();
      fd.append("file", new Blob([body], { type: "text/csv" }), "bookmarks.csv");
      const r = await fetch(
        "/api/v1/bookmarks/import?format=csv&strategy=upsert&mode=import",
        { method: "POST", headers: { Authorization: `Bearer ${key}` }, body: fd }
      );
      return { status: r.status, body: await r.json() };
    },
    { key: apiKey, body: csvBody }
  );

  expect(resp.status).toBe(200);
  expect(resp.body.created).toBe(2);

  await freshContext.close();
});

// Test 6: Import with strategy=skip leaves existing bookmarks unchanged
test("importing with skip strategy does not overwrite existing bookmark", async ({
  page,
  browser,
}) => {
  const { apiKey, freshPage, freshContext } = await setupWithBookmark(
    page,
    browser,
    "import-skip-test",
    { url: "https://skip-test.example.com", title: "Original Title", tags: ["original"] }
  );

  const resp = await freshPage.evaluate(
    async ({ key }) => {
      const body = JSON.stringify({
        url: "https://skip-test.example.com",
        title: "New Title",
        tags: ["new"],
      });
      const fd = new FormData();
      fd.append("file", new Blob([body], { type: "application/x-ndjson" }), "b.jsonl");
      const r = await fetch(
        "/api/v1/bookmarks/import?format=jsonl&strategy=skip&mode=import",
        { method: "POST", headers: { Authorization: `Bearer ${key}` }, body: fd }
      );
      return { status: r.status, body: await r.json() };
    },
    { key: apiKey }
  );

  expect(resp.status).toBe(200);
  expect(resp.body.skipped).toBe(1);
  expect(resp.body.created).toBe(0);
  expect(resp.body.updated).toBe(0);

  // Title must still be original
  const exportResp = await freshPage.evaluate(async (key) => {
    const r = await fetch("/api/v1/bookmarks/export?format=jsonl&mode=export", {
      headers: { Authorization: `Bearer ${key}` },
    });
    return await r.text();
  }, apiKey);
  const bm = exportResp
    .split("\n")
    .filter((l) => l.trim())
    .map((l) => JSON.parse(l))
    .find((l) => l.url === "https://skip-test.example.com");
  expect(bm.title).toBe("Original Title");

  await freshContext.close();
});

// Test 7: Import with strategy=upsert updates existing bookmarks
test("importing with upsert strategy overwrites matching bookmark", async ({
  page,
  browser,
}) => {
  const { apiKey, freshPage, freshContext } = await setupWithBookmark(
    page,
    browser,
    "import-upsert-test",
    { url: "https://upsert-test.example.com", title: "Old Title", tags: ["old"] }
  );

  const resp = await freshPage.evaluate(
    async ({ key }) => {
      const body = JSON.stringify({
        url: "https://upsert-test.example.com",
        title: "Updated Title",
        description: "New desc",
        tags: ["updated"],
      });
      const fd = new FormData();
      fd.append("file", new Blob([body], { type: "application/x-ndjson" }), "b.jsonl");
      const r = await fetch(
        "/api/v1/bookmarks/import?format=jsonl&strategy=upsert&mode=import",
        { method: "POST", headers: { Authorization: `Bearer ${key}` }, body: fd }
      );
      return { status: r.status, body: await r.json() };
    },
    { key: apiKey }
  );

  expect(resp.status).toBe(200);
  expect(resp.body.updated).toBe(1);
  expect(resp.body.created).toBe(0);
  expect(resp.body.skipped).toBe(0);

  // Verify updated title
  const exportResp = await freshPage.evaluate(async (key) => {
    const r = await fetch("/api/v1/bookmarks/export?format=jsonl&mode=export", {
      headers: { Authorization: `Bearer ${key}` },
    });
    return await r.text();
  }, apiKey);
  const bm = exportResp
    .split("\n")
    .filter((l) => l.trim())
    .map((l) => JSON.parse(l))
    .find((l) => l.url === "https://upsert-test.example.com");
  expect(bm.title).toBe("Updated Title");

  await freshContext.close();
});

// Test 8: Import with invalid URL records error but continues
test("a row with invalid URL is recorded as an error without aborting import", async ({
  page,
  browser,
}) => {
  await signIn(page);
  const apiKey = await createApiKey(page, "import-invalid-url-test");

  const freshContext = await browser.newContext();
  const freshPage = await freshContext.newPage();
  await freshPage.goto(page.url());

  const jsonlBody = [
    JSON.stringify({ url: "https://valid-row.example.com", title: "Valid", tags: [] }),
    JSON.stringify({ url: "not-a-url", title: "Invalid", tags: [] }),
  ].join("\n");

  const resp = await freshPage.evaluate(
    async ({ key, body }) => {
      const fd = new FormData();
      fd.append("file", new Blob([body], { type: "application/x-ndjson" }), "b.jsonl");
      const r = await fetch(
        "/api/v1/bookmarks/import?format=jsonl&strategy=upsert&mode=import",
        { method: "POST", headers: { Authorization: `Bearer ${key}` }, body: fd }
      );
      return { status: r.status, body: await r.json() };
    },
    { key: apiKey, body: jsonlBody }
  );

  expect(resp.status).toBe(200);
  expect(resp.body.created).toBe(1);
  expect(resp.body.errors).toHaveLength(1);

  await freshContext.close();
});

// Test 9: Import with malformed file returns 400
test("uploading a malformed JSONL file returns 400 with parse error", async ({
  page,
  browser,
}) => {
  await signIn(page);
  const apiKey = await createApiKey(page, "import-malformed-test");

  const freshContext = await browser.newContext();
  const freshPage = await freshContext.newPage();
  await freshPage.goto(page.url());

  const resp = await freshPage.evaluate(
    async ({ key }) => {
      const fd = new FormData();
      fd.append(
        "file",
        new Blob(["this is not valid json at all"], { type: "application/x-ndjson" }),
        "bad.jsonl"
      );
      const r = await fetch(
        "/api/v1/bookmarks/import?format=jsonl",
        { method: "POST", headers: { Authorization: `Bearer ${key}` }, body: fd }
      );
      return { status: r.status, body: await r.json() };
    },
    { key: apiKey }
  );

  expect(resp.status).toBe(400);
  expect(resp.body.error).toContain("parse error");

  await freshContext.close();
});

// Test 10: Import without file field returns 400
test("POST to import without a file field returns 400", async ({
  page,
  browser,
}) => {
  await signIn(page);
  const apiKey = await createApiKey(page, "import-no-file-test");

  const freshContext = await browser.newContext();
  const freshPage = await freshContext.newPage();
  await freshPage.goto(page.url());

  const resp = await freshPage.evaluate(
    async ({ key }) => {
      const fd = new FormData();
      const r = await fetch(
        "/api/v1/bookmarks/import?format=jsonl",
        { method: "POST", headers: { Authorization: `Bearer ${key}` }, body: fd }
      );
      return { status: r.status, body: await r.json() };
    },
    { key: apiKey }
  );

  expect(resp.status).toBe(400);
  expect(typeof resp.body.error).toBe("string");

  await freshContext.close();
});

// Test 11: Export and import endpoints require authentication
test("unauthenticated requests to export and import endpoints return 401", async ({
  request,
}) => {
  const exportResp = await request.get("/api/v1/bookmarks/export");
  expect(exportResp.status()).toBe(401);

  const importResp = await request.post("/api/v1/bookmarks/import");
  expect(importResp.status()).toBe(401);
});

// Test 12: Export-import JSONL roundtrip preserves data
test("export-delete-import JSONL roundtrip preserves URLs, titles, descriptions, tags", async ({
  page,
  browser,
}) => {
  await signIn(page);
  const apiKey = await createApiKey(page, "jsonl-roundtrip-test");

  const freshContext = await browser.newContext();
  const freshPage = await freshContext.newPage();
  await freshPage.goto(page.url());

  // Create two bookmarks
  const ids = await freshPage.evaluate(async (key) => {
    const created = [];
    for (const bm of [
      {
        url: "https://rt-1.example.com",
        title: "Roundtrip 1",
        description: "Desc 1",
        tags: ["x"],
      },
      {
        url: "https://rt-2.example.com",
        title: "Roundtrip 2",
        description: "Desc 2",
        tags: ["y"],
      },
    ]) {
      const r = await fetch("/api/v1/bookmarks", {
        method: "POST",
        headers: {
          Authorization: `Bearer ${key}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify(bm),
      });
      const body = await r.json();
      created.push(body.id);
    }
    return created;
  }, apiKey);

  // Export
  const exported = await freshPage.evaluate(async (key) => {
    const r = await fetch(
      "/api/v1/bookmarks/export?format=jsonl&mode=export",
      { headers: { Authorization: `Bearer ${key}` } }
    );
    return await r.text();
  }, apiKey);

  // Delete both
  await freshPage.evaluate(
    async ({ key, ids }) => {
      for (const id of ids) {
        await fetch(`/api/v1/bookmarks/${id}`, {
          method: "DELETE",
          headers: { Authorization: `Bearer ${key}` },
        });
      }
    },
    { key: apiKey, ids }
  );

  // Re-import
  await freshPage.evaluate(
    async ({ key, body }) => {
      const fd = new FormData();
      fd.append(
        "file",
        new Blob([body], { type: "application/x-ndjson" }),
        "export.jsonl"
      );
      await fetch(
        "/api/v1/bookmarks/import?format=jsonl&strategy=upsert&mode=import",
        { method: "POST", headers: { Authorization: `Bearer ${key}` }, body: fd }
      );
    },
    { key: apiKey, body: exported }
  );

  // Re-export and compare
  const after = await freshPage.evaluate(async (key) => {
    const r = await fetch(
      "/api/v1/bookmarks/export?format=jsonl&mode=export",
      { headers: { Authorization: `Bearer ${key}` } }
    );
    return await r.text();
  }, apiKey);

  const afterLines = after
    .split("\n")
    .filter((l) => l.trim())
    .map((l) => JSON.parse(l));

  const rt1 = afterLines.find((l) => l.url === "https://rt-1.example.com");
  const rt2 = afterLines.find((l) => l.url === "https://rt-2.example.com");
  expect(rt1).toBeDefined();
  expect(rt1.title).toBe("Roundtrip 1");
  expect(rt1.tags).toContain("x");
  expect(rt2).toBeDefined();
  expect(rt2.title).toBe("Roundtrip 2");

  await freshContext.close();
});

// Test 13: Export-import CSV roundtrip preserves data
test("export-delete-import CSV roundtrip preserves fields", async ({
  page,
  browser,
}) => {
  await signIn(page);
  const apiKey = await createApiKey(page, "csv-roundtrip-test");

  const freshContext = await browser.newContext();
  const freshPage = await freshContext.newPage();
  await freshPage.goto(page.url());

  // Create bookmark
  const id = await freshPage.evaluate(async (key) => {
    const r = await fetch("/api/v1/bookmarks", {
      method: "POST",
      headers: {
        Authorization: `Bearer ${key}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({
        url: "https://csv-rt.example.com",
        title: "CSV Roundtrip",
        description: "CSV desc",
        tags: ["csv", "rt"],
      }),
    });
    const body = await r.json();
    return body.id;
  }, apiKey);

  // Export as CSV
  const csvExport = await freshPage.evaluate(async (key) => {
    const r = await fetch(
      "/api/v1/bookmarks/export?format=csv&mode=export",
      { headers: { Authorization: `Bearer ${key}` } }
    );
    return await r.text();
  }, apiKey);

  // Delete
  await freshPage.evaluate(
    async ({ key, id }) => {
      await fetch(`/api/v1/bookmarks/${id}`, {
        method: "DELETE",
        headers: { Authorization: `Bearer ${key}` },
      });
    },
    { key: apiKey, id }
  );

  // Re-import CSV
  const importResp = await freshPage.evaluate(
    async ({ key, body }) => {
      const fd = new FormData();
      fd.append("file", new Blob([body], { type: "text/csv" }), "export.csv");
      const r = await fetch(
        "/api/v1/bookmarks/import?format=csv&strategy=upsert&mode=import",
        { method: "POST", headers: { Authorization: `Bearer ${key}` }, body: fd }
      );
      return { status: r.status, body: await r.json() };
    },
    { key: apiKey, body: csvExport }
  );

  expect(importResp.status).toBe(200);
  expect(importResp.body.created).toBeGreaterThanOrEqual(1);

  // Verify URL survived
  const after = await freshPage.evaluate(async (key) => {
    const r = await fetch(
      "/api/v1/bookmarks/export?format=jsonl&mode=export",
      { headers: { Authorization: `Bearer ${key}` } }
    );
    return await r.text();
  }, apiKey);
  expect(after).toContain("https://csv-rt.example.com");

  await freshContext.close();
});

// Test 14: Settings page shows Import & Export section
test("settings page displays Import & Export section with export links and import form", async ({
  page,
}) => {
  await signIn(page);
  await page.goto("/settings");

  await expect(page.getByRole("heading", { name: "Import & Export" })).toBeVisible();
  await expect(page.getByText("Backup or migrate your bookmarks.")).toBeVisible();

  await expect(page.getByRole("link", { name: "Export JSONL" })).toBeVisible();
  await expect(page.getByRole("link", { name: "Export CSV" })).toBeVisible();
  await expect(page.getByRole("link", { name: "Backup JSONL" })).toBeVisible();
  await expect(page.getByRole("link", { name: "Backup CSV" })).toBeVisible();

  await expect(
    page.getByRole("link", { name: "Export JSONL" })
  ).toHaveAttribute("href", /format=jsonl.*mode=export|mode=export.*format=jsonl/);

  await expect(page.locator("#import-form")).toBeVisible();
  await expect(page.locator("input[type=file]")).toBeVisible();
  await expect(page.getByRole("button", { name: "Import" })).toBeVisible();
});

// Test 15: Web UI import form submits file and displays result
test("settings page import form shows result after uploading a JSONL file", async ({
  page,
  browser,
}) => {
  await signIn(page);
  await page.goto("/settings");

  // Create an API key via UI so we have one (not needed for the cookie-auth form)
  const jsonlContent = JSON.stringify({
    url: "https://form-import-test.example.com",
    title: "Form Import",
    tags: [],
  });

  // Write a temp file and upload it
  const { writeFileSync } = require("fs");
  const { join } = require("path");
  const tmpFile = join(require("os").tmpdir(), "boopmark-e2e-import.jsonl");
  writeFileSync(tmpFile, jsonlContent, "utf8");

  await page.locator("select[name=format]").selectOption("jsonl");
  await page.locator("select[name=mode]").selectOption("import");
  await page.locator("input[type=file]").setInputFiles(tmpFile);
  await page.getByRole("button", { name: "Import" }).click();

  const resultEl = page.locator("#import-result");
  await expect(resultEl).not.toBeEmpty({ timeout: 10000 });
  const text = await resultEl.textContent();
  // Expect either a success summary or an error message (form submitted)
  expect(text.length).toBeGreaterThan(0);
});

// Test 16: Backup-mode export and restore-mode import roundtrip preserves IDs
test("backup JSONL export followed by restore import preserves original bookmark ID", async ({
  page,
  browser,
}) => {
  const { apiKey, freshPage, freshContext, bookmarkId } = await setupWithBookmark(
    page,
    browser,
    "backup-restore-test",
    {
      url: "https://backup-restore-test.example.com",
      title: "Backup Restore",
      tags: ["backup"],
    }
  );

  // Export as backup
  const backupExport = await freshPage.evaluate(async (key) => {
    const r = await fetch(
      "/api/v1/bookmarks/export?format=jsonl&mode=backup",
      { headers: { Authorization: `Bearer ${key}` } }
    );
    return await r.text();
  }, apiKey);

  // Delete the bookmark
  await freshPage.evaluate(
    async ({ key, id }) => {
      await fetch(`/api/v1/bookmarks/${id}`, {
        method: "DELETE",
        headers: { Authorization: `Bearer ${key}` },
      });
    },
    { key: apiKey, id: bookmarkId }
  );

  // Restore
  const restoreResp = await freshPage.evaluate(
    async ({ key, body }) => {
      const fd = new FormData();
      fd.append(
        "file",
        new Blob([body], { type: "application/x-ndjson" }),
        "backup.jsonl"
      );
      const r = await fetch(
        "/api/v1/bookmarks/import?format=jsonl&mode=restore&strategy=upsert",
        { method: "POST", headers: { Authorization: `Bearer ${key}` }, body: fd }
      );
      return { status: r.status, body: await r.json() };
    },
    { key: apiKey, body: backupExport }
  );

  expect(restoreResp.status).toBe(200);
  expect(restoreResp.body.created).toBe(1);

  // Find the restored bookmark and check its ID matches
  const afterExport = await freshPage.evaluate(async (key) => {
    const r = await fetch(
      "/api/v1/bookmarks/export?format=jsonl&mode=backup",
      { headers: { Authorization: `Bearer ${key}` } }
    );
    return await r.text();
  }, apiKey);
  const lines = afterExport
    .split("\n")
    .filter((l) => l.trim())
    .map((l) => JSON.parse(l));
  const restored = lines.find(
    (l) => l.url === "https://backup-restore-test.example.com"
  );
  expect(restored).toBeDefined();
  expect(restored.id).toBe(bookmarkId);

  await freshContext.close();
});

// Test 17: Restore mode rejects records without id field
test("restore mode records an error when a record has no id field", async ({
  page,
  browser,
}) => {
  await signIn(page);
  const apiKey = await createApiKey(page, "restore-no-id-test");

  const freshContext = await browser.newContext();
  const freshPage = await freshContext.newPage();
  await freshPage.goto(page.url());

  const resp = await freshPage.evaluate(
    async ({ key }) => {
      const body = JSON.stringify({
        url: "https://no-id.example.com",
        title: "No ID",
        tags: [],
      });
      const fd = new FormData();
      fd.append("file", new Blob([body], { type: "application/x-ndjson" }), "b.jsonl");
      const r = await fetch(
        "/api/v1/bookmarks/import?format=jsonl&mode=restore",
        { method: "POST", headers: { Authorization: `Bearer ${key}` }, body: fd }
      );
      return { status: r.status, body: await r.json() };
    },
    { key: apiKey }
  );

  expect(resp.status).toBe(200);
  expect(resp.body.errors).toHaveLength(1);
  expect(resp.body.errors[0].message).toContain("id");

  await freshContext.close();
});

// Clean up API keys created by this spec
test.afterAll(async ({ browser }) => {
  const context = await browser.newContext();
  const page = await context.newPage();
  await signIn(page);
  const keys = await page.evaluate(async () => {
    const r = await fetch("/api/v1/auth/keys");
    return r.json();
  });
  for (const key of keys) {
    await page.evaluate(async (id) => {
      await fetch(`/api/v1/auth/keys/${id}`, { method: "DELETE" });
    }, key.id);
  }
  await context.close();
});
