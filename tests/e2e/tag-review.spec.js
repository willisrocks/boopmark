const { test, expect } = require("@playwright/test");
const crypto = require("node:crypto");

// The E2E server sends every LLM request to scripts/e2e/fake-llm.mjs, which
// calls js ~ javascript and k8s ~ kubernetes the same tag at high confidence
// and wfh ~ remote work at medium.

async function signIn(page) {
  await page.goto("/");
  await page.getByRole("button", { name: "Sign in for E2E" }).click();
  await expect(page).toHaveURL(/\/bookmarks$/);
}

async function enableFakeProvider(page) {
  await page.goto("/settings");
  const enable = page.getByLabel("Enable LLM integration");
  if (!(await enable.isChecked())) {
    await enable.check();
  }
  await page.getByTestId("metadata-provider").selectOption("anthropic");
  if (await page.getByLabel("Anthropic API key").count()) {
    await page.getByLabel("Anthropic API key").fill("sk-ant-e2e-fake-key");
  }
  await page.getByRole("button", { name: "Save settings" }).click();
  await expect(page).toHaveURL(/\/settings\?saved=1$/);
}

// Restore mode stores tags verbatim, so the library can hold spelling
// duplicates that the normal write path would fold.
async function seedLibrary(page) {
  const records = [
    ["javascript", "Closures in JavaScript"],
    ["js", "JS array tips"],
    ["k8s", "Pods explained"],
    ["kubernetes", "Kubernetes networking"],
    ["wfh", "Desk setup for home"],
    ["remote work", "Async teams"],
    ["GitHub", "GitHub Actions caching"],
    ["github", "Code review on github"],
  ].map(([tag, title]) => ({
    id: crypto.randomUUID(),
    url: `https://tags.example.com/${encodeURIComponent(tag)}`,
    title,
    tags: [tag],
  }));
  await importRecords(page, records);
}

async function importRecords(page, records) {
  const response = await page.request.post(
    "/api/v1/bookmarks/import?format=jsonl&mode=restore&strategy=upsert",
    {
      multipart: {
        file: {
          name: "seed.jsonl",
          mimeType: "application/x-ndjson",
          buffer: Buffer.from(records.map((r) => JSON.stringify(r)).join("\n")),
        },
      },
    },
  );
  expect(response.status()).toBe(200);
  expect((await response.json()).errors).toHaveLength(0);
}

async function tagsByTitle(page) {
  const response = await page.request.get("/api/v1/bookmarks");
  expect(response.status()).toBe(200);
  const bookmarks = await response.json();
  return Object.fromEntries(bookmarks.map((b) => [b.title, b.tags]));
}

const row = (page, canonical) =>
  page.locator(`[data-testid="review-row"][data-canonical="${canonical}"]`);

// Other specs share the E2E user; dismiss whatever they left to review.
async function clearSuggestions(page) {
  await page.goto("/tags/review");
  await page.getByTestId("review-run-normalize").click();
  await expect(page.getByTestId("review-notice")).toBeVisible();
  const boxes = page.locator('[data-testid="review-row"] input[name="ids"]');
  if (await boxes.count()) {
    for (const box of await boxes.all()) {
      await box.check();
    }
    await page.getByTestId("review-dismiss").click();
    await expect(page.getByTestId("review-notice")).toContainText("Dismissed");
  }
  await expect(page.getByTestId("review-empty")).toBeVisible();
}

test("the review screen applies only the checked merges and undoes them as one batch", async ({ page }) => {
  await signIn(page);
  await enableFakeProvider(page);
  await clearSuggestions(page);
  await seedLibrary(page);
  const before = await tagsByTitle(page);

  await page.goto("/tags/review");
  await page.getByTestId("review-run-ai").click();

  // The run happens in the background; the page polls and reloads.
  await expect(page.getByTestId("review-row")).toHaveCount(4, { timeout: 20_000 });
  await expect(page.getByTestId("consolidation-status")).toHaveAttribute("data-state", "succeeded");
  // Finding duplicates changed no tag.
  expect(await tagsByTitle(page)).toEqual(before);

  // "Check these" comes first and is unchecked; high rows are pre-checked.
  const checkThese = page.getByTestId("review-check-these");
  await expect(checkThese.getByTestId("review-row")).toHaveCount(1);
  await expect(row(page, "remote work").locator('input[name="ids"]')).not.toBeChecked();
  const high = page.getByTestId("review-high");
  await expect(high.getByTestId("review-row")).toHaveCount(3);
  for (const canonical of ["javascript", "kubernetes", "github"]) {
    await expect(row(page, canonical).locator('input[name="ids"]')).toBeChecked();
  }
  await expect(row(page, "javascript").getByTestId("review-row-members")).toContainText("js");
  await expect(page.getByTestId("review-apply")).toHaveText("Apply 3 merges");

  // Keep kubernetes apart, accept the "Check these" rows, and rename the
  // GitHub merge target.
  await row(page, "kubernetes").locator('input[name="ids"]').uncheck();
  await page.getByTestId("review-select-all").click();
  await expect(row(page, "remote work").locator('input[name="ids"]')).toBeChecked();
  await row(page, "github").getByTestId("review-row-canonical").fill("GitHub");
  await expect(page.getByTestId("review-apply")).toHaveText("Apply 3 merges");
  await page.getByTestId("review-apply").click();

  await expect(page.getByTestId("review-notice")).toContainText("Applied 3 merges");
  await expect(page.getByTestId("review-row")).toHaveCount(1);
  await expect(row(page, "kubernetes")).toBeVisible();

  const applied = await tagsByTitle(page);
  expect(applied["JS array tips"]).toEqual(["javascript"]);
  expect(applied["Desk setup for home"]).toEqual(["remote work"]);
  expect(applied["Code review on github"]).toEqual(["GitHub"]);
  expect(applied["GitHub Actions caching"]).toEqual(["GitHub"]);
  // The unchecked row did not apply.
  expect(applied["Pods explained"]).toEqual(["k8s"]);
  expect(applied["Kubernetes networking"]).toEqual(["kubernetes"]);

  // One click undoes the whole batch.
  await page.getByTestId("review-undo-applied").click();
  await expect(page.getByTestId("review-notice")).toContainText("Undone");
  expect(await tagsByTitle(page)).toEqual(before);
  await expect(page.getByTestId("review-row")).toHaveCount(4);
});

test("without a provider key the AI run is disabled but spelling variants still work", async ({ page }) => {
  await signIn(page);
  await page.goto("/settings");
  const deleteKey = page.getByTestId("delete-anthropic-api-key");
  if (await deleteKey.count()) {
    await deleteKey.check();
    await page.getByRole("button", { name: "Save settings" }).click();
    await expect(page).toHaveURL(/\/settings\?saved=1$/);
  }

  await page.goto("/tags/review");
  await expect(page.getByTestId("review-run-ai")).toBeDisabled();
  await page.getByTestId("review-run-normalize").click();
  await expect(page.getByTestId("review-notice")).toBeVisible();
});

test("a spelling that differs only by spaces shows its spaces", async ({ page }) => {
  await signIn(page);
  await clearSuggestions(page);
  // HTML collapses spaces, so without a marker " zig " would look like "zig".
  await importRecords(
    page,
    ["zig", " zig "].map((tag, i) => ({
      id: crypto.randomUUID(),
      url: `https://tags.example.com/zig-${i}`,
      title: `Zig ${i}`,
      tags: [tag],
    })),
  );

  await page.goto("/tags/review");
  await page.getByTestId("review-run-normalize").click();
  const chips = row(page, "zig").getByTestId("review-chip");
  await expect(chips).toHaveCount(2);
  const padded = chips.filter({ hasText: "␠zig␠" });
  await expect(padded).toHaveCount(1);
  await expect(padded).toHaveAttribute("title", "“ zig ”");
  const plain = chips.filter({ hasNotText: "␠" });
  await expect(plain).toHaveCount(1);
  await expect(plain).not.toHaveAttribute("title");
});
