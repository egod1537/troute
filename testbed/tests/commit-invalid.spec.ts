import { test, expect } from "@playwright/test";

test("invalid build commit renders unknown without a link", async ({
  page,
}) => {
  await page.route("**/api/health", (route) =>
    route.fulfill({ json: { status: "ok" } }),
  );

  await page.goto("/");

  await expect(page.getByLabel("commit 알 수 없음")).toContainText(
    "commit 알 수 없음",
  );
  await expect(
    page.getByRole("link", { name: /Commit/i }),
  ).toHaveCount(0);
  await expect(page.getByLabel("API 정상")).toBeVisible();
  await expect(
    page.getByRole("button", { name: "API 상태 새로고침" }),
  ).toBeVisible();
});
