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
  const rawKey = await page.getByTestId("api-key-raw-value").textContent();
  const keys = await page.evaluate(async () => {
    const response = await fetch("/api/v1/auth/keys");
    return response.json();
  });
  return { rawKey, id: keys.find((key) => key.name === name).id };
}

test("bookmark create idempotency is enforced through the HTTP API", async ({
  page,
  browser,
}) => {
  await signIn(page);
  let apiKey;
  let context;
  let apiPage;
  let results;
  try {
    apiKey = await createApiKey(page, `idempotency-${Date.now()}`);
    context = await browser.newContext();
    apiPage = await context.newPage();
    await apiPage.goto(page.url());

    results = await apiPage.evaluate(async (key) => {
      const operationKey = crypto.randomUUID();
      const run = crypto.randomUUID();
      const payload = {
        url: `https://example.com/idempotency-${run}`,
        title: "Reviewed title",
        description: "Reviewed description",
        image_url: "https://example.com/reviewed.png",
        domain: "example.com",
        tags: ["reviewed", "idempotency"],
      };
      const create = async (body, idempotencyKey) => {
        const headers = {
          Authorization: `Bearer ${key}`,
          "Content-Type": "application/json",
        };
        if (idempotencyKey !== undefined) {
          headers["Idempotency-Key"] = idempotencyKey;
        }
        const response = await fetch("/api/v1/bookmarks", {
          method: "POST",
          headers,
          body: JSON.stringify(body),
        });
        return { status: response.status, body: await response.json() };
      };

      const first = await create(payload, operationKey);
      const replay = await create(payload, operationKey);
      const mismatch = await create(
        { ...payload, title: "Changed reviewed title" },
        operationKey
      );
      const invalid = await create(payload, "not-a-uuid");
      const headerlessFirst = await create(payload);
      const headerlessSecond = await create(payload);

      return {
        first,
        replay,
        mismatch,
        invalid,
        headerlessFirst,
        headerlessSecond,
      };
    }, apiKey.rawKey);

    expect(results.first.status).toBe(201);
    expect(results.first.body.id).toBeDefined();
    expect(results.replay.status).toBe(201);
    expect(results.replay.body.id).toBe(results.first.body.id);
    expect(results.mismatch.status).toBe(409);
    expect(results.invalid.status).toBe(400);
    expect(results.headerlessFirst.status).toBe(201);
    expect(results.headerlessSecond.status).toBe(201);
    expect(results.headerlessFirst.body.id).not.toBe(
      results.headerlessSecond.body.id
    );
  } finally {
    if (results) {
      const ids = new Set([
        results.first.body.id,
        results.headerlessFirst.body.id,
        results.headerlessSecond.body.id,
      ].filter(Boolean));
      for (const id of ids) {
        await apiPage.evaluate(async ({ key, id }) => {
          await fetch(`/api/v1/bookmarks/${id}`, {
            method: "DELETE",
            headers: { Authorization: `Bearer ${key}` },
          });
        }, { key: apiKey.rawKey, id });
      }
    }
    if (context) await context.close();
    if (apiKey) {
      await page.evaluate(async (id) => {
        await fetch(`/api/v1/auth/keys/${id}`, { method: "DELETE" });
      }, apiKey.id);
    }
  }
});
