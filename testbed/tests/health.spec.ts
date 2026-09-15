import { test, expect } from "@playwright/test";

test("health-only mode shows API status, raw response, and latest latency", async ({
  page,
}) => {
  let healthCalls = 0;
  await page.route("**/api/health", async (route) => {
    healthCalls++;
    await new Promise((resolve) => setTimeout(resolve, 40));
    await route.fulfill({ json: { status: "ok" } });
  });

  await page.goto("/");
  await expect(page.getByLabel("API Online")).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Request", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Response", exact: true }),
  ).toBeVisible();
  await expect(page.getByText("HTTP 200", { exact: true })).toBeVisible();
  await expect(page.getByText(/\d+\.\d ms/)).toBeVisible();
  await expect(page.getByLabel("Raw API response")).toContainText(
    '"status": "ok"',
  );
  await expect(page.getByRole("table")).toHaveCount(0);
  await expect(page.getByText("Performance", { exact: true })).toHaveCount(0);

  await page.getByRole("button", { name: "Run", exact: true }).click();
  await expect.poll(() => healthCalls).toBe(2);
  await expect(page.getByLabel("API Online")).toBeVisible();

  await page.getByRole("button", { name: "Refresh API health" }).click();
  await expect.poll(() => healthCalls).toBe(3);
  await expect(page.getByLabel("API Online")).toBeVisible();
});

test("invalid JSON is visible and the sample can be restored", async ({
  page,
}) => {
  await page.route("**/api/health", (route) =>
    route.fulfill({ json: { status: "ok" } }),
  );
  await page.goto("/");
  await expect(page.getByLabel("API Online")).toBeVisible();

  await page.getByLabel("Request JSON").fill("{ broken");
  await page.getByRole("button", { name: "Validate", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("Invalid JSON");

  await page.getByRole("button", { name: "Reset sample" }).click();
  await page.getByRole("button", { name: "Validate", exact: true }).click();
  await expect(page.getByText("Valid", { exact: true })).toBeVisible();
});

test("HTTP failures retain status, latency, and response body", async ({
  page,
}) => {
  await page.route("**/api/health", (route) =>
    route.fulfill({ status: 503, json: { error: "upstream unavailable" } }),
  );
  await page.goto("/");

  await expect(page.getByLabel("API Offline")).toBeVisible();
  await expect(page.getByRole("alert")).toContainText("HTTP 503");
  await expect(page.getByText("HTTP 503", { exact: true })).toBeVisible();
  await expect(page.getByText(/\d+\.\d ms/)).toBeVisible();
  await expect(page.getByLabel("Raw API response")).toContainText(
    "upstream unavailable",
  );
});

test("malformed health responses remain inspectable", async ({ page }) => {
  await page.route("**/api/health", (route) =>
    route.fulfill({
      body: "<html>not the API</html>",
      contentType: "text/html",
    }),
  );
  await page.goto("/");

  await expect(page.getByLabel("API Offline")).toBeVisible();
  await expect(page.getByRole("alert")).toContainText("Malformed response");
  await expect(page.getByLabel("Raw API response")).toContainText(
    "not the API",
  );
});

test("connection errors are visible without stale response data", async ({
  page,
}) => {
  await page.route("**/api/health", (route) =>
    route.abort("connectionrefused"),
  );
  await page.goto("/");

  await expect(page.getByRole("alert")).toContainText("API connection failed");
  await expect(page.getByLabel("API Offline")).toBeVisible();
  await expect(page.getByLabel("Raw API response")).toContainText(
    "No response yet",
  );
});

test("mobile layout stacks request before response without horizontal overflow", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.route("**/api/health", (route) =>
    route.fulfill({ json: { status: "ok" } }),
  );
  await page.goto("/");

  await expect(page.getByLabel("API Online")).toBeVisible();
  await expect(page.getByLabel("Request JSON")).toBeVisible();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);

  const requestTop = await page
    .getByRole("heading", { name: "Request", exact: true })
    .evaluate((element) => element.getBoundingClientRect().top);
  const responseTop = await page
    .getByRole("heading", { name: "Response", exact: true })
    .evaluate((element) => element.getBoundingClientRect().top);
  expect(requestTop).toBeLessThan(responseTop);
});
