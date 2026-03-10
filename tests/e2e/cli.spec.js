const { test, expect } = require("@playwright/test");
const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const BASE = "http://127.0.0.1:4010";
const API = `${BASE}/api/v1`;
const BOOP = path.resolve("target/debug/boop");

let configDir;
let apiKey;

// Helper: run the boop CLI binary with the isolated config dir
function boop(...args) {
  return execFileSync(BOOP, args, {
    env: { ...process.env, BOOP_CONFIG_DIR: configDir },
    encoding: "utf-8",
    timeout: 30_000,
  });
}

// Helper: run boop and expect it to fail (non-zero exit)
function boopFails(...args) {
  try {
    execFileSync(BOOP, args, {
      env: { ...process.env, BOOP_CONFIG_DIR: configDir },
      encoding: "utf-8",
      timeout: 30_000,
    });
    throw new Error("Expected boop to fail but it succeeded: boop " + args.join(" "));
  } catch (err) {
    if (err.status === undefined || err.status === null) throw err;
    return { stderr: err.stderr || "", stdout: err.stdout || "", exitCode: err.status };
  }
}

// Helper: get a session token via the JSON test-token endpoint
async function getSessionToken(request) {
  const resp = await request.post(`${API}/auth/test-token`);
  expect(resp.status()).toBe(200);
  const body = await resp.json();
  expect(body.session_token).toBeTruthy();
  return body.session_token;
}

test.describe("CLI E2E", () => {
  test.beforeAll(async ({ request }) => {
    // Build the CLI binary
    execFileSync("cargo", ["build", "-p", "boop"], {
      encoding: "utf-8",
      timeout: 120_000,
    });

    // Create isolated config directory
    configDir = fs.mkdtempSync(path.join(os.tmpdir(), "boop-e2e-"));

    // Get an API key
    const sessionToken = await getSessionToken(request);
    const resp = await request.post(`${API}/auth/keys`, {
      headers: { Cookie: `session=${sessionToken}` },
      data: { name: "cli-e2e" },
    });
    expect(resp.status()).toBe(201);
    const body = await resp.json();
    apiKey = body.key;

    // Configure the CLI
    boop("config", "set-server", BASE);
    boop("config", "set-key", apiKey);
  });

  test.afterAll(() => {
    if (configDir) {
      fs.rmSync(configDir, { recursive: true, force: true });
    }
  });

  test("config show displays server and key", () => {
    const output = boop("config", "show");
    expect(output).toContain("Server:");
    expect(output).toContain("API Key:");
  });

  test("full bookmark CRUD lifecycle", () => {
    // Add
    const addOutput = boop("add", "https://example.com/cli-test", "--title", "CLI Test", "--tags", "cli,test");
    expect(addOutput).toContain("Added:");

    // List (JSON) and extract the ID
    const listJson = boop("--output", "json", "list");
    const bookmarks = JSON.parse(listJson);
    const created = bookmarks.find((b) => b.title === "CLI Test");
    expect(created).toBeTruthy();
    const id = created.id;

    // Get
    const getOutput = boop("get", id);
    expect(getOutput).toContain("CLI Test");

    // Get (JSON)
    const getJson = boop("--output", "json", "get", id);
    const fetched = JSON.parse(getJson);
    expect(fetched.title).toBe("CLI Test");

    // Update
    const updateOutput = boop("update", id, "--title", "Updated CLI Test", "--tags", "cli,test,updated");
    expect(updateOutput).toContain("Updated");

    // Verify update via JSON get
    const updatedJson = boop("--output", "json", "get", id);
    const updated = JSON.parse(updatedJson);
    expect(updated.title).toBe("Updated CLI Test");

    // List (plain) should contain updated title
    const listPlain = boop("list");
    expect(listPlain).toContain("Updated CLI Test");

    // Search
    const searchOutput = boop("search", "Updated");
    expect(searchOutput).toContain("Updated CLI Test");

    // Tags
    const tagsOutput = boop("tags");
    expect(tagsOutput).toContain("cli");

    // Tags (JSON)
    const tagsJson = boop("--output", "json", "tags");
    const tags = JSON.parse(tagsJson);
    expect(tags.some((t) => t.name === "cli")).toBe(true);

    // Delete
    const deleteOutput = boop("delete", id);
    expect(deleteOutput).toContain("Deleted");

    // Verify deleted (should fail)
    const result = boopFails("get", id);
    expect(result.exitCode).not.toBe(0);
  });

  test("get nonexistent bookmark fails", () => {
    const result = boopFails("get", "00000000-0000-0000-0000-000000000000");
    expect(result.exitCode).not.toBe(0);
  });
});
