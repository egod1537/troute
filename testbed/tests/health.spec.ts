import { expect, test } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  await page.route("**/api/integration/jobs?limit=50", (route) =>
    route.fulfill({ json: { jobs: [] } }),
  );
});

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
      const bounds = element.getBoundingClientRect();
      return bounds.width >= 16 && bounds.height >= 10;
    }),
  ).toBe(true);
  await expect(page.locator('link[rel="icon"]')).toHaveAttribute(
    "href",
    "/troute-icon.svg",
  );
  await expect(page).toHaveTitle("troute · 테스트베드");
  await expect(page.locator("html")).toHaveAttribute("lang", "ko");
  const favicon = await page.request.get("/troute-icon.svg");
  expect(favicon.ok()).toBe(true);
  expect(favicon.headers()["content-type"]).toContain("image/svg+xml");
  const faviconSvg = await favicon.text();
  expect(faviconSvg).toContain('stroke="#2D72D2"');
  expect(faviconSvg).not.toContain("currentColor");
  await expect(page.getByText("troute 테스트베드")).toBeVisible();
  await expect(page.getByLabel("API 정상")).toBeVisible();
  await expect(page.getByLabel("commit 알 수 없음")).toContainText(
    "commit 알 수 없음",
  );
  await expect(
    page.getByRole("heading", { name: "Jobs", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText("실행 정보를 확인할 Job을 선택하세요."),
  ).toBeVisible();
  await expect(page.getByText("아직 Job이 없습니다")).toBeVisible();
  await expect(page.getByRole("button", { name: "새 Job" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Request" })).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "Response" })).toHaveCount(0);

  await page.getByRole("button", { name: "API 상태 새로고침" }).click();
  await expect.poll(() => healthCalls).toBe(2);
  await expect(page.getByLabel("API 정상")).toBeVisible();
});

test("New Job dialog validates JSON and regenerates the sample id", async ({
  page,
}) => {
  await page.route("**/api/health", (route) =>
    route.fulfill({ json: { status: "ok" } }),
  );
  await page.goto("/");
  await page.getByRole("button", { name: "새 Job" }).click();

  const dialog = page.getByRole("dialog", { name: "새 Job" });
  const editor = dialog.getByLabel("요청 JSON");
  await expect(dialog).toBeVisible();
  const initialRequest = JSON.parse(await editor.inputValue());
  const firstId = initialRequest.job_id;
  expect(initialRequest.locations).toHaveLength(3);
  await expect(
    dialog.getByText(/locations\[0\].*고정 start/),
  ).toBeVisible();

  await editor.fill("{ broken");
  await dialog.getByRole("button", { name: "검증" }).click();
  await expect(dialog.getByRole("alert")).toContainText("JSON 형식 오류");

  await dialog.getByRole("button", { name: "샘플 복원" }).click();
  const secondId = JSON.parse(await editor.inputValue()).job_id;
  expect(secondId).not.toBe(firstId);
  await dialog.getByRole("button", { name: "검증" }).click();
  await expect(dialog.getByText("유효함", { exact: true })).toBeVisible();

  const compact = JSON.stringify(JSON.parse(await editor.inputValue()));
  await editor.fill(compact);
  await dialog.getByRole("button", { name: "포맷" }).click();
  await expect(editor).toHaveValue(/\n  "job_id"/);
});

test("invalid schema blocks job creation", async ({ page }) => {
  await page.route("**/api/health", (route) =>
    route.fulfill({ json: { status: "ok" } }),
  );
  await page.goto("/");
  await page.getByRole("button", { name: "새 Job" }).click();
  const dialog = page.getByRole("dialog", { name: "새 Job" });
  await dialog.getByLabel("요청 JSON").fill("{}");
  await dialog.getByRole("button", { name: "Job 생성" }).click();

  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("alert")).toContainText("요청에는 job_id");
  await expect(page.getByRole("option")).toHaveCount(0);
});

test("offline health remains visible in the preserved header", async ({ page }) => {
  await page.route("**/api/health", (route) =>
    route.fulfill({ status: 503, json: { error: "upstream unavailable" } }),
  );
  await page.goto("/");
  await expect(page.getByLabel("API 연결 실패")).toBeVisible();
  await expect(
    page.getByRole("button", { name: "API 상태 새로고침" }),
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

  await expect(page.getByLabel("API 정상")).toBeVisible();
  expect(
    await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth),
  ).toBe(true);

  const jobsTop = await page
    .getByRole("heading", { name: "Jobs", exact: true })
    .evaluate((element) => element.getBoundingClientRect().top);
  const detailTop = await page
    .getByText("실행 정보를 확인할 Job을 선택하세요.")
    .evaluate((element) => element.getBoundingClientRect().top);
  expect(jobsTop).toBeLessThan(detailTop);
});
