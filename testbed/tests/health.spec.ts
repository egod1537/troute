import { expect, test } from "@playwright/test";

test("health-only shell preserves header controls and job-centric layout", async ({
  page,
}) => {
  let healthCalls = 0;
  await page.route("**/api/health", async (route) => {
    healthCalls += 1;
    await route.fulfill({ json: { status: "ok" } });
  });

  await page.goto("/");
  const icon = page.locator(".navbar-brand .troute-icon");
  await expect(icon).toBeVisible();
  expect(
    await icon.evaluate((element) => {
      const bounds = (element as SVGGraphicsElement).getBBox();
      return bounds.width >= 16 && bounds.height >= 10;
    }),
  ).toBe(true);
  await expect(page.locator('link[rel="icon"]')).toHaveAttribute(
    "href",
    "/troute-icon.svg",
  );
  await expect(page.getByLabel("API Online")).toBeVisible();
  await expect(page.getByLabel("Commit unknown")).toContainText(
    "commit unknown",
  );
  await expect(
    page.getByRole("heading", { name: "Jobs", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText("Select a job to inspect its execution."),
  ).toBeVisible();
  await expect(page.getByRole("button", { name: "New Job" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Request" })).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "Response" })).toHaveCount(0);

  await page.getByRole("button", { name: "Refresh API health" }).click();
  await expect.poll(() => healthCalls).toBe(2);
  await expect(page.getByLabel("API Online")).toBeVisible();
});

test("New Job dialog validates JSON and regenerates the sample id", async ({
  page,
}) => {
  await page.route("**/api/health", (route) =>
    route.fulfill({ json: { status: "ok" } }),
  );
  await page.goto("/");
  await page.getByRole("button", { name: "New Job" }).click();

  const dialog = page.getByRole("dialog", { name: "New Job" });
  const editor = dialog.getByLabel("Request JSON");
  await expect(dialog).toBeVisible();
  const firstId = JSON.parse(await editor.inputValue()).job_id;

  await editor.fill("{ broken");
  await dialog.getByRole("button", { name: "Validate" }).click();
  await expect(dialog.getByRole("alert")).toContainText("Invalid JSON");

  await dialog.getByRole("button", { name: "Reset sample" }).click();
  const secondId = JSON.parse(await editor.inputValue()).job_id;
  expect(secondId).not.toBe(firstId);
  await dialog.getByRole("button", { name: "Validate" }).click();
  await expect(dialog.getByText("Valid", { exact: true })).toBeVisible();

  const compact = JSON.stringify(JSON.parse(await editor.inputValue()));
  await editor.fill(compact);
  await dialog.getByRole("button", { name: "Format" }).click();
  await expect(editor).toHaveValue(/\n  "job_id"/);
});

test("invalid schema blocks job creation", async ({ page }) => {
  await page.route("**/api/health", (route) =>
    route.fulfill({ json: { status: "ok" } }),
  );
  await page.goto("/");
  await page.getByRole("button", { name: "New Job" }).click();
  const dialog = page.getByRole("dialog", { name: "New Job" });
  await dialog.getByLabel("Request JSON").fill("{}");
  await dialog.getByRole("button", { name: "Create Job" }).click();

  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("alert")).toContainText("Input requires");
  await expect(page.getByRole("option")).toHaveCount(0);
});

test("offline health remains visible in the preserved header", async ({ page }) => {
  await page.route("**/api/health", (route) =>
    route.fulfill({ status: 503, json: { error: "upstream unavailable" } }),
  );
  await page.goto("/");
  await expect(page.getByLabel("API Offline")).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Refresh API health" }),
  ).toBeVisible();
});

test("mobile layout stacks jobs before detail without horizontal overflow", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.route("**/api/health", (route) =>
    route.fulfill({ json: { status: "ok" } }),
  );
  await page.goto("/");

  await expect(page.getByLabel("API Online")).toBeVisible();
  expect(
    await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth),
  ).toBe(true);

  const jobsTop = await page
    .getByRole("heading", { name: "Jobs", exact: true })
    .evaluate((element) => element.getBoundingClientRect().top);
  const detailTop = await page
    .getByText("Select a job to inspect its execution.")
    .evaluate((element) => element.getBoundingClientRect().top);
  expect(jobsTop).toBeLessThan(detailTop);
});
