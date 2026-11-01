const { test, expect } = require("@playwright/test");

// The compact tag filter bar and the /tags library page. The tests share one
// seeded library and run in order: the merge test leaves an alias that the
// search test relies on, and the undo test reverts it.
test.describe.configure({ mode: "serial" });

// 15 tags: rust 5; python, javascript, docker 3; linux, kubernetes, security 2;
// the rest 1. So the bar shows 10 chips and "More tags (5)".
const BOOKMARKS = [
  ["rust", "docker", "linux"],
  ["rust", "python", "databases"],
  ["rust", "javascript", "security"],
  ["rust", "docker", "kubernetes"],
  ["python", "javascript", "testing"],
  ["python", "writing", "design"],
  ["javascript", "docker", "kubernetes"],
  ["go", "linux", "security"],
  ["k8s", "cooking"],
  ["music", "rust"],
];

async function signIn(page) {
  await page.goto("/");
  await page.getByRole("button", { name: "Sign in for E2E" }).click();
  await expect(page).toHaveURL(/\/bookmarks$/);
}

// Other specs may have saved bookmarks for the shared E2E account; start
// from an empty library so tag counts are exact.
async function clearBookmarks(page) {
  for (;;) {
    const html = await (await page.request.get("/bookmarks")).text();
    const ids = [...html.matchAll(/hx-delete="\/bookmarks\/([0-9a-f-]{36})"/g)].map((m) => m[1]);
    if (ids.length === 0) return;
    for (const id of ids) {
      expect((await page.request.delete(`/bookmarks/${id}`)).ok()).toBeTruthy();
    }
  }
}

async function openTagSearch(page, query) {
  await page.getByTestId("more-tags-button").click();
  const input = page.getByTestId("tag-search-input");
  await expect(input).toBeFocused();
  await input.fill(query);
  return input;
}

// The tags query parameter of the current URL, as htmx pushed it.
async function expectTagFilter(page, tags) {
  await expect.poll(() => new URL(page.url()).searchParams.get("tags")).toBe(tags);
}

const cards = (page) => page.getByTestId("bookmark-card");
const row = (page, name) => page.locator(`[data-testid="tag-row"][data-tag-name="${name}"]`);

test.beforeAll(async ({ browser }) => {
  const page = await browser.newPage();
  await signIn(page);
  await clearBookmarks(page);
  for (const [i, tags] of BOOKMARKS.entries()) {
    // Port 9 refuses connections at once, so metadata lookups fail fast.
    const response = await page.request.post("/bookmarks", {
      form: { url: `http://127.0.0.1:9/tags-e2e/${i}`, title: `Tag test ${i}`, tags_input: tags.join(", ") },
    });
    expect(response.ok()).toBeTruthy();
  }
  await page.close();
});

test("the filter bar shows the top tags, a More popover and multi-select", async ({ page }) => {
  await signIn(page);

  const chips = page.getByTestId("filter-tag-chip");
  await expect(chips).toHaveCount(10);
  await expect(chips.first()).toHaveText(/rust\s*5/);
  await expect(page.getByTestId("more-tags-button")).toHaveText(/More tags \(5\)/);
  const bar = await page.getByTestId("filter-bar").boundingBox();
  expect(bar.height).toBeLessThan(70);

  await chips.filter({ hasText: "kubernetes" }).click();
  await expectTagFilter(page, "kubernetes");
  await expect(page.getByTestId("active-tag-chip")).toHaveText(/kubernetes/);
  await expect(cards(page)).toHaveCount(2);

  // The popover adds to the filter instead of replacing it.
  await openTagSearch(page, "cook");
  await expect(page.getByTestId("tag-option")).toHaveText([/cooking\s*1/]);
  await page.keyboard.press("Enter");
  await expectTagFilter(page, "kubernetes,cooking");
  await expect(page.getByTestId("active-tag-chip")).toHaveCount(2);
  await expect(cards(page)).toHaveCount(3);
  await expect(page.getByTestId("tag-popover")).toBeHidden();

  // Arrow keys move through results; Escape closes the popover.
  await openTagSearch(page, "s");
  const options = page.getByTestId("tag-option");
  await expect(options.first()).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("ArrowDown");
  await expect(options.nth(1)).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("tag-popover")).toBeHidden();

  await page.getByTestId("active-tag-chip").filter({ hasText: "kubernetes" }).click();
  await expectTagFilter(page, "cooking");
  await expect(cards(page)).toHaveCount(1);

  // The sort control keeps the tag filter.
  await page.locator("select[name='sort']").selectOption("title");
  await expect(cards(page)).toHaveCount(1);
  await expect(page.getByTestId("active-tag-chip")).toHaveText(/cooking/);
});

test("a merge on /tags makes the old name an alias that the filter search finds", async ({ page }) => {
  await signIn(page);
  await page.getByTestId("profile-menu-trigger").hover();
  await page.getByTestId("profile-menu-tags").click();
  await expect(page).toHaveURL(/\/tags$/);
  await expect(page.getByTestId("tag-library-summary")).toContainText("15 tags");
  await expect(page.getByTestId("tag-review-link")).toHaveAttribute("href", "/tags/review");

  await row(page, "k8s").getByTestId("tag-merge").click();
  await expect(page.getByTestId("tag-merge-dialog")).toBeVisible();
  await page.getByTestId("tag-merge-target").fill("kubernetes");
  await page.getByTestId("tag-merge-submit").click();

  await expect(page.getByTestId("tag-flash")).toContainText("Merged “k8s” into “kubernetes” on 1 bookmark.");
  await expect(page.getByTestId("tag-merge-dialog")).toBeHidden();
  await expect(row(page, "k8s")).toHaveCount(0);
  await expect(row(page, "kubernetes").getByTestId("tag-row-count")).toHaveText("3");
  await expect(row(page, "kubernetes").getByTestId("tag-row-aliases")).toHaveText("k8s");
  await expect(page.getByTestId("tag-batch")).toHaveCount(1);

  await page.goto("/bookmarks");
  await openTagSearch(page, "k8s");
  await expect(page.getByTestId("tag-option")).toHaveText([/kubernetes\s*also: k8s\s*3/]);
  await page.keyboard.press("Enter");
  await expectTagFilter(page, "kubernetes");
  await expect(cards(page)).toHaveCount(3);
});

test("undo restores the merged tag", async ({ page }) => {
  await signIn(page);
  await page.goto("/tags");
  await expect(row(page, "k8s")).toHaveCount(0);

  await page.getByTestId("tag-batch-undo").click();
  await expect(page.getByTestId("tag-flash")).toContainText("Undid merge: k8s → kubernetes.");
  await expect(row(page, "k8s").getByTestId("tag-row-count")).toHaveText("1");
  await expect(row(page, "kubernetes").getByTestId("tag-row-count")).toHaveText("2");
  await expect(page.getByTestId("tag-batch")).toContainText("Undone");
  await expect(page.getByTestId("tag-batch-undo")).toHaveCount(0);
});

test("rename, bulk merge, filter and sort on /tags", async ({ page }) => {
  await signIn(page);
  await page.goto("/tags");

  await row(page, "writing").getByTestId("tag-rename").click();
  await expect(page.getByTestId("tag-merge-target")).toHaveValue("writing");
  await page.getByTestId("tag-merge-target").fill("Technical Writing");
  await page.getByTestId("tag-merge-submit").click();
  await expect(page.getByTestId("tag-flash")).toContainText("Renamed “writing” to “Technical Writing” on 1 bookmark.");
  await expect(row(page, "Technical Writing").getByTestId("tag-row-aliases")).toHaveText("writing");

  // Undo from the flash right after the change.
  await page.getByTestId("tag-flash-undo").click();
  await expect(page.getByTestId("tag-flash")).toContainText("Undid rename");
  await expect(row(page, "writing")).toHaveCount(1);
  await expect(row(page, "Technical Writing")).toHaveCount(0);

  await row(page, "design").locator("[data-tag-select]").check();
  await row(page, "testing").locator("[data-tag-select]").check();
  await expect(page.getByTestId("tag-bulk-bar")).toContainText("2 selected");
  await page.getByTestId("tag-merge-selected").click();
  await expect(page.getByTestId("tag-merge-dialog")).toContainText("Merge 2 tags into…");
  await page.getByTestId("tag-merge-target").fill("craft");
  await page.getByTestId("tag-merge-submit").click();
  await expect(page.getByTestId("tag-flash")).toContainText("Merged “design”, “testing” into “craft” on 2 bookmarks.");
  await expect(row(page, "craft").getByTestId("tag-row-count")).toHaveText("2");
  await expect(page.getByTestId("tag-bulk-bar")).toBeHidden();

  // A merge into itself is refused with a message, not an error page.
  await row(page, "rust").getByTestId("tag-merge").click();
  await page.getByTestId("tag-merge-target").fill("rust");
  await page.getByTestId("tag-merge-submit").click();
  await expect(page.getByTestId("tag-flash")).toContainText("Choose tags to merge into a different tag.");

  // The filter box matches aliases too; the sort keeps the filter.
  await page.getByTestId("tag-library-filter").fill("sign");
  await expect(page.getByTestId("tag-row")).toHaveCount(1);
  await expect(page.getByTestId("tag-row")).toHaveAttribute("data-tag-name", "craft");
  await page.getByTestId("tag-library-filter").fill("");
  await page.getByTestId("tag-library-sort").selectOption("az");
  await expect(page).toHaveURL(/sort=az/);
  await expect(page.getByTestId("tag-row").first()).toHaveAttribute("data-tag-name", "cooking");
  await page.getByTestId("tag-library-sort").selectOption("least");
  await expect(page.getByTestId("tag-row").last()).toHaveAttribute("data-tag-name", "rust");
});

test("the filter bar and /tags fit a phone screen", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await signIn(page);

  await expect(page.getByTestId("filter-tag-chip").filter({ visible: true })).toHaveCount(4);
  const bar = await page.getByTestId("filter-bar").boundingBox();
  expect(bar.height).toBeLessThan(70);
  const noSideScroll = () => page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth);
  expect(await noSideScroll()).toBe(true);

  await openTagSearch(page, "k");
  const popover = await page.getByTestId("tag-popover").boundingBox();
  expect(popover.x).toBeGreaterThanOrEqual(0);
  expect(popover.x + popover.width).toBeLessThanOrEqual(390);

  await page.goto("/tags");
  await expect(page.getByTestId("tag-row").first()).toBeVisible();
  expect(await noSideScroll()).toBe(true);
});
