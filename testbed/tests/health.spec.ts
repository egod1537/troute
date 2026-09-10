import { test, expect } from "@playwright/test";

test("health-only UI shows real request timing without claiming a solver result", async ({
  page,
}) => {
  await page.route("**/api/health", async (route) => {
    await new Promise((resolve) => setTimeout(resolve, 40));
    await route.fulfill({ json: { status: "ok" } });
  });
  await page.goto("/");
  await expect(page.getByText("API: Online")).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "No route has been calculated" }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Run route", exact: true }),
  ).toHaveCount(0);
  await expect(page.getByLabel("Raw API response")).toContainText(
    '"status": "ok"',
  );
  await page.getByRole("button", { name: "Run health check" }).click();
  await expect(
    page.getByRole("img", {
      name: /Request latency chart, 2 session requests/,
    }),
  ).toBeVisible();
  await expect(
    page.getByText("input payload is not submitted", { exact: false }),
  ).toBeVisible();
  await page.reload();
  await expect(
    page.getByRole("img", {
      name: /Request latency chart, 1 session requests/,
    }),
  ).toBeVisible();
});

test("invalid JSON is visible and sample can be restored", async ({ page }) => {
  await page.route("**/api/health", (route) =>
    route.fulfill({ json: { status: "ok" } }),
  );
  await page.goto("/");
  await expect(page.getByText("API: Online")).toBeVisible();
  await page.getByLabel("Route input JSON").fill("{ broken");
  await page.getByRole("button", { name: "Validate JSON" }).click();
  await expect(page.getByRole("alert")).toContainText("Invalid JSON");
  await page.getByRole("button", { name: "Load sample" }).click();
  await page.getByRole("button", { name: "Validate JSON" }).click();
  await expect(page.getByRole("status")).toContainText("Valid v0 input shape");
});

test("HTTP failures retain status and response body", async ({ page }) => {
  await page.route("**/api/health", (route) =>
    route.fulfill({ status: 503, json: { error: "upstream unavailable" } }),
  );
  await page.goto("/");
  await expect(page.getByText("API: Offline")).toBeVisible();
  await expect(page.getByRole("alert")).toContainText("HTTP 503");
  await expect(page.getByLabel("Raw API response")).toContainText(
    "upstream unavailable",
  );
});

test("malformed health response is not shown as online", async ({ page }) => {
  await page.route("**/api/health", (route) =>
    route.fulfill({
      body: "<html>not the API</html>",
      contentType: "text/html",
    }),
  );
  await page.goto("/");
  await expect(page.getByText("API: Offline")).toBeVisible();
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
  await expect(page.getByText("API: Offline")).toBeVisible();
  await expect(page.getByLabel("Raw API response")).toContainText(
    "No response received",
  );
});

test("mobile layout keeps the editor and controls within the viewport", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.route("**/api/health", (route) =>
    route.fulfill({ json: { status: "ok" } }),
  );
  await page.goto("/");
  await expect(page.getByText("API: Online")).toBeVisible();
  await expect(page.getByLabel("Route input JSON")).toBeVisible();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
});
