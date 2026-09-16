import { expect, test, type Page } from "@playwright/test";

interface ServerJob {
  id: string;
  status: "pending" | "running" | "completed" | "failed" | "cancelled";
  createdAt: number;
  updatedAt: number;
  progress: number;
}

const requestFor = (jobId: string) => ({
  job_id: jobId,
  locations: [
    {
      id: "A",
      place_id: "place-a",
      open_time: "00:00",
      close_time: "23:59",
      stay_minutes: 0,
    },
    {
      id: "B",
      place_id: "place-b",
      open_time: "00:00",
      close_time: "23:59",
      stay_minutes: 0,
    },
  ],
  start_time: "09:00",
});

function recordFor(job: ServerJob) {
  return {
    request: requestFor(job.id),
    state: {
      job_id: job.id,
      status: job.status,
      stage: job.status === "running" ? "solving" : job.status,
      progress: job.progress,
      last_message: `${job.status} message`,
      created_at: job.createdAt,
      updated_at: job.updatedAt,
      completed_at:
        job.status === "completed" ||
        job.status === "failed" ||
        job.status === "cancelled"
          ? job.updatedAt
          : null,
    },
    result: null,
    error: null,
  };
}

async function routeServerJobs(
  page: Page,
  getJobs: () => ServerJob[],
  onList?: () => void,
) {
  await page.route("**/api/health", (route) =>
    route.fulfill({ json: { status: "ok" } }),
  );
  await page.route("**/api/integration/jobs?limit=50", (route) => {
    onList?.();
    return route.fulfill({
      json: {
        jobs: getJobs().map((job) => ({
          job_id: job.id,
          status: job.status,
          created_at: job.createdAt,
          updated_at: job.updatedAt,
        })),
      },
    });
  });
  await page.route(
    /\/api\/integration\/jobs\/[^/?]+\/timeline$/,
    (route) => {
      const jobId = decodeURIComponent(
        new URL(route.request().url()).pathname.split("/").at(-2)!,
      );
      return route.fulfill({ json: { job_id: jobId, entries: [] } });
    },
  );
  await page.route(/\/api\/integration\/jobs\/[^/?]+$/, (route) => {
    const jobId = decodeURIComponent(
      new URL(route.request().url()).pathname.split("/").at(-1)!,
    );
    const job = getJobs().find((candidate) => candidate.id === jobId);
    return job
      ? route.fulfill({ json: recordFor(job) })
      : route.fulfill({ status: 404, json: { error: "not found" } });
  });
}

test("polling merges new and changed Jobs while preserving selection", async ({
  page,
}) => {
  let listCalls = 0;
  let jobs: ServerJob[] = [
    {
      id: "job-a",
      status: "completed",
      createdAt: 1_789_521_000_000,
      updatedAt: 1_789_521_001_000,
      progress: 100,
    },
  ];
  await routeServerJobs(page, () => jobs, () => {
    listCalls += 1;
  });

  await page.goto("/");
  const jobA = page.getByRole("option", { name: /job-a/ });
  await expect(jobA).toBeVisible();
  await expect(jobA).toHaveAttribute("aria-selected", "true");

  jobs = [
    {
      id: "job-b",
      status: "running",
      createdAt: 1_789_522_000_000,
      updatedAt: 1_789_522_001_000,
      progress: 55,
    },
    jobs[0],
  ];
  await expect.poll(() => listCalls, { timeout: 3_500 }).toBeGreaterThanOrEqual(2);
  const jobB = page.getByRole("option", { name: /job-b/ });
  await expect(page.getByRole("option").first()).toContainText("job-b");
  await expect(jobB).toContainText("실행 중");
  await expect(jobB).toContainText("55%");
  await expect(jobA).toHaveAttribute("aria-selected", "true");

  jobs = [
    {
      ...jobs[0],
      status: "cancelled",
      updatedAt: 1_789_522_002_000,
      progress: 60,
    },
    jobs[1],
  ];
  await page.getByRole("button", { name: "Job 목록 새로고침" }).click();
  await expect(jobB).toHaveAttribute("data-status", "cancelled");
  await expect(jobB).toContainText("취소됨");
  await expect(jobB).toContainText("60%");
  await expect(jobA).toHaveAttribute("aria-selected", "true");

  jobs = [jobs[0]];
  await page.getByRole("button", { name: "Job 목록 새로고침" }).click();
  await expect(jobA).toHaveCount(0);
  await expect(jobB).toHaveAttribute("aria-selected", "true");
});

test("refresh indicator blocks duplicate requests and preserves rows on failure", async ({
  page,
}) => {
  let listCalls = 0;
  let releaseInitial!: () => void;
  let releaseFailure!: () => void;
  const initialGate = new Promise<void>((resolve) => {
    releaseInitial = resolve;
  });
  const failureGate = new Promise<void>((resolve) => {
    releaseFailure = resolve;
  });
  const job: ServerJob = {
    id: "job-preserved",
    status: "completed",
    createdAt: 1_789_521_000_000,
    updatedAt: 1_789_521_001_000,
    progress: 100,
  };
  await page.route("**/api/health", (route) =>
    route.fulfill({ json: { status: "ok" } }),
  );
  await page.route("**/api/integration/jobs?limit=50", async (route) => {
    listCalls += 1;
    if (listCalls === 1) await initialGate;
    if (listCalls === 2) {
      await failureGate;
      return route.fulfill({ status: 503, json: { error: "unavailable" } });
    }
    return route.fulfill({
      json: {
        jobs: [
          {
            job_id: job.id,
            status: job.status,
            created_at: job.createdAt,
            updated_at: job.updatedAt,
          },
        ],
      },
    });
  });
  await page.route(/\/api\/integration\/jobs\/job-preserved$/, (route) =>
    route.fulfill({ json: recordFor(job) }),
  );
  await page.route(
    "**/api/integration/jobs/job-preserved/timeline",
    (route) => route.fulfill({ json: { job_id: job.id, entries: [] } }),
  );

  await page.goto("/");
  const refreshing = page.getByRole("button", {
    name: "Job 목록 새로고침 중",
  });
  await expect(refreshing).toHaveClass(/is-refreshing/);
  await expect(refreshing).toBeDisabled();
  await expect(refreshing.locator("svg")).toHaveCSS(
    "animation-name",
    "jobs-refresh-spin",
  );
  await refreshing.evaluate((button: HTMLButtonElement) => button.click());
  expect(listCalls).toBe(1);

  releaseInitial();
  await expect(
    page.getByRole("button", { name: "Job 목록 새로고침" }),
  ).toBeEnabled();
  const row = page.getByRole("option", { name: /job-preserved/ });
  await expect(row).toBeVisible();

  await page.getByRole("button", { name: "Job 목록 새로고침" }).click();
  await expect(refreshing).toBeVisible();
  expect(listCalls).toBe(2);
  releaseFailure();
  const failed = page.getByRole("button", {
    name: "Job 목록 새로고침 실패. 다시 시도",
  });
  await expect(failed).toBeEnabled();
  await expect(row).toBeVisible();

  await failed.click();
  await expect.poll(() => listCalls).toBe(3);
  await expect(
    page.getByRole("button", { name: "Job 목록 새로고침" }),
  ).toBeEnabled();
});

test("selected active Job detail refreshes independently every second", async ({
  page,
}) => {
  let listCalls = 0;
  let job: ServerJob = {
    id: "job-active",
    status: "running",
    createdAt: 1_789_521_000_000,
    updatedAt: 1_789_521_001_000,
    progress: 10,
  };
  await routeServerJobs(page, () => [job], () => {
    listCalls += 1;
  });

  await page.goto("/");
  const row = page.getByRole("option", { name: /job-active/ });
  await expect(row).toContainText("10%");
  job = {
    ...job,
    updatedAt: 1_789_521_002_000,
    progress: 40,
  };

  await expect(row).toContainText("40%", { timeout: 1_700 });
  expect(listCalls).toBe(1);
});

test("hidden tabs skip polling and refresh immediately when visible", async ({
  page,
}) => {
  await page.addInitScript(() => {
    let testVisibility: DocumentVisibilityState = "visible";
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      get: () => testVisibility,
    });
    (
      window as typeof window & {
        __setTestVisibility?: (value: DocumentVisibilityState) => void;
      }
    ).__setTestVisibility = (value) => {
      testVisibility = value;
      document.dispatchEvent(new Event("visibilitychange"));
    };
  });
  let listCalls = 0;
  await routeServerJobs(page, () => [], () => {
    listCalls += 1;
  });

  await page.goto("/");
  await expect.poll(() => listCalls).toBe(1);
  await page.evaluate(() => {
    (
      window as typeof window & {
        __setTestVisibility: (value: DocumentVisibilityState) => void;
      }
    ).__setTestVisibility("hidden");
  });
  await page.waitForTimeout(2_300);
  expect(listCalls).toBe(1);

  await page.evaluate(() => {
    (
      window as typeof window & {
        __setTestVisibility: (value: DocumentVisibilityState) => void;
      }
    ).__setTestVisibility("visible");
  });
  await expect.poll(() => listCalls).toBe(2);
});

test("reduced motion disables refresh rotation without hiding state", async ({
  page,
}) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  let release!: () => void;
  const gate = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.route("**/api/health", (route) =>
    route.fulfill({ json: { status: "ok" } }),
  );
  await page.route("**/api/integration/jobs?limit=50", async (route) => {
    await gate;
    return route.fulfill({ json: { jobs: [] } });
  });

  await page.goto("/");
  const indicator = page.getByRole("button", {
    name: "Job 목록 새로고침 중",
  });
  await expect(indicator).toHaveClass(/is-refreshing/);
  await expect(indicator.locator("svg")).toHaveCSS("animation-name", "none");
  release();
  await expect(
    page.getByRole("button", { name: "Job 목록 새로고침" }),
  ).toBeEnabled();
});
