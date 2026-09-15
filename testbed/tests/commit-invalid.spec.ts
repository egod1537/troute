import { test, expect } from "@playwright/test";

test("invalid build commit renders unknown without a link", async ({
  page,
}) => {
  await page.route("**/api/health", (route) =>
    route.fulfill({ json: { status: "ok" } }),
  );

  await page.goto("/");

  await expect(page.getByLabel("Commit unknown")).toContainText(
    "commit unknown",
  );
  await expect(
    page.getByRole("link", { name: /Commit/i }),
  ).toHaveCount(0);
  await expect(page.getByLabel("API Online")).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Refresh API health" }),
  ).toBeVisible();
});
