import { expect, test, type Page } from "@playwright/test";

const routeResult = {
  route: [
    {
      order: 0,
      location_id: "A",
      arrival_time: "09:00",
      departure_time: "09:00",
    },
    {
      order: 1,
      location_id: "B",
      arrival_time: "09:25",
      departure_time: "11:30",
    },
    { order: 2, location_id: "C", arrival_time: "12:15" },
  ],
  total_travel_minutes: 70,
};

test.beforeEach(async ({ page }) => {
  await page.route("**/api/health", (route) =>
    route.fulfill({ json: { status: "ok" } }),
  );
});

async function openJobDialog(page: Page, jobId?: string) {
  await page.getByRole("button", { name: "새 Job" }).click();
  const dialog = page.getByRole("dialog", { name: "새 Job" });
  const editor = dialog.getByLabel("요청 JSON");
  if (jobId) {
    const input = JSON.parse(await editor.inputValue());
    input.job_id = jobId;
    await editor.fill(JSON.stringify(input, null, 2));
  }
  return { dialog, editor };
}

async function createJob(page: Page, jobId: string) {
  const { dialog } = await openJobDialog(page, jobId);
  await dialog.getByRole("button", { name: "Job 생성" }).click();
}

test("valid build commit and API health controls remain in the header", async ({
  page,
}) => {
  await page.goto("/");

  const commit = page.getByRole("link", {
    name: "commit 3f4f8d25686fe955582295e1e47334c7a277c681",
  });
  await expect(commit).toContainText("commit 3f4f8d2");
  await expect(commit).toHaveAttribute(
    "href",
    "https://github.com/egod1537/troute/commit/3f4f8d25686fe955582295e1e47334c7a277c681",
  );
  await expect(page.getByLabel("API 정상")).toBeVisible();
  await expect(
    page.getByRole("button", { name: "API 상태 새로고침" }),
  ).toBeVisible();
});

test("job is inserted and selected before the request completes, then succeeds", async ({
  page,
}) => {
  let releaseRoute!: () => void;
  const routeGate = new Promise<void>((resolve) => {
    releaseRoute = resolve;
  });
  await page.route("**/api/fixture-route", async (route) => {
    expect(route.request().method()).toBe("POST");
    expect(route.request().postDataJSON().job_id).toBe("route-immediate");
    await routeGate;
    await route.fulfill({ json: routeResult });
  });
  await page.goto("/");

  await page.evaluate(() => {
    const seen: string[] = [];
    (window as typeof window & { __jobStatuses?: string[] }).__jobStatuses = seen;
    new MutationObserver(() => {
      document.querySelectorAll<HTMLElement>("[data-status]").forEach((item) => {
        const status = item.dataset.status;
        if (status && !seen.includes(status)) seen.push(status);
      });
    }).observe(document.body, { childList: true, subtree: true, attributes: true });
  });

  await createJob(page, "route-immediate");
  const row = page.getByRole("option", { name: /route-immediate/ });
  await expect(row).toBeVisible();
  await expect(row).toHaveAttribute("aria-selected", "true");
  await expect(page.getByRole("heading", { name: "Job 상세" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "route-immediate" })).toBeVisible();
  await expect(row).toHaveAttribute("data-status", "running");
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as typeof window & { __jobStatuses?: string[] })
            .__jobStatuses ?? [],
      ),
    )
    .toContain("pending");

  releaseRoute();
  await expect(row).toHaveAttribute("data-status", "completed");
  await expect(row).toContainText("완료");
  await expect(page.getByLabel("방문 순서")).toHaveText("A → B → C");
  await expect(page.getByRole("table")).toContainText("09:25");
  await expect(page.getByText("총 이동 시간", { exact: true })).toBeVisible();
  await expect(page.getByText("70분", { exact: true })).toBeVisible();
  await expect(page.getByText("HTTP 200", { exact: true })).toBeVisible();
  await expect(page.getByText(/\d+\.\d ms/)).toBeVisible();
  await expect(page.getByLabel("Raw API 응답")).toContainText(
    "total_travel_minutes",
  );
  await page.getByRole("button", { name: "복사", exact: true }).click();
  await expect(page.getByRole("button", { name: "복사됨" })).toBeVisible();
});

test("duplicate ids are rejected and newest jobs sort first", async ({ page }) => {
  await page.route("**/api/fixture-route", (route) =>
    route.fulfill({ json: routeResult }),
  );
  await page.goto("/");

  await createJob(page, "route-first");
  await expect(
    page.getByRole("option", { name: /route-first/ }),
  ).toHaveAttribute("data-status", "completed");

  const duplicate = await openJobDialog(page, "route-first");
  await duplicate.dialog.getByRole("button", { name: "Job 생성" }).click();
  await expect(duplicate.dialog.getByRole("alert")).toContainText(
    "이 세션에 이미 존재합니다",
  );
  await expect(page.getByRole("option", { name: /route-first/ })).toHaveCount(1);
  await duplicate.dialog.getByRole("button", { name: "취소" }).click();

  await createJob(page, "route-second");
  await expect(
    page.getByRole("option", { name: /route-second/ }),
  ).toHaveAttribute("data-status", "completed");

  await expect(page.getByRole("option").nth(0)).toContainText("route-second");
  await expect(page.getByRole("option").nth(1)).toContainText("route-first");
  await expect(
    page.getByRole("heading", { name: "route-second" }),
  ).toBeVisible();

  await page.getByRole("option", { name: /route-first/ }).click();
  await expect(
    page.getByRole("heading", { name: "route-first" }),
  ).toBeVisible();
  await expect(
    page.getByRole("option", { name: /route-first/ }),
  ).toHaveAttribute("aria-selected", "true");
});

test("request failure transitions the selected job to failed", async ({ page }) => {
  await page.route("**/api/fixture-route", (route) =>
    route.fulfill({
      status: 422,
      json: { error: "infeasible route: visit window exceeded" },
    }),
  );
  await page.goto("/");

  await createJob(page, "route-failed");
  const row = page.getByRole("option", { name: /route-failed/ });
  await expect(row).toHaveAttribute("data-status", "failed");
  await expect(row).toContainText("오류");
  await expect(page.getByRole("alert")).toContainText("HTTP 422");
  await expect(page.getByText("HTTP 422", { exact: true })).toBeVisible();
  await expect(page.getByLabel("Raw API 응답")).toContainText(
    "infeasible route",
  );
});

test("malformed successful responses fail without losing inspection data", async ({
  page,
}) => {
  await page.route("**/api/fixture-route", (route) =>
    route.fulfill({ json: { route: "incorrect shape" } }),
  );
  await page.goto("/");

  await createJob(page, "route-malformed");
  await expect(
    page.getByRole("option", { name: /route-malformed/ }),
  ).toHaveAttribute("data-status", "failed");
  await expect(page.getByRole("alert")).toContainText("경로 응답 형식 오류");
  await expect(page.getByLabel("Raw API 응답")).toContainText(
    "incorrect shape",
  );
  await expect(page.getByRole("table")).toHaveCount(0);
});
