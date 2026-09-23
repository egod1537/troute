import { expect, test, type Page } from "@playwright/test";

function successfulSaCandidate(strategy: string, initialStrategy: string) {
  return {
    strategy,
    best: false,
    route: ["A", "B", "C"],
    feasible: true,
    objective_score: {
      latest_start: "08:55",
      finish_time: "12:19",
      travel_minutes: 74,
      wait_minutes: 1,
    },
    elapsed_ms: 5,
    metadata: {
      initial_strategy: initialStrategy,
      initial_route: ["A", "B", "C"],
      final_route: ["A", "B", "C"],
      initial_score: 75,
      final_score: 75,
      initial_temperature: "1000",
      final_temperature: "605.7704364907278",
      cooling_rate: "0.995",
      iteration_count: 100,
      accepted_moves: 25,
      improved_moves: 8,
      swap_move_count: 34,
      relocate_move_count: 33,
      two_opt_move_count: 33,
      accepted_worse_moves: 5,
      infeasible_candidates: 2,
      accepted_infeasible_moves: 1,
      best_feasible: true,
      seed: 42,
      timed_out: false,
    },
  };
}

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
  solver_candidates: [
    {
      strategy: "exact_bit_dp",
      best: true,
      route: ["A", "B", "C"],
      feasible: true,
      objective_score: {
        latest_start: "09:00",
        finish_time: "12:15",
        travel_minutes: 70,
        wait_minutes: 0,
      },
      elapsed_ms: 7,
      metadata: {
        state_count: 18,
        frontier_state_count: 8,
        frontier_cell_count: 6,
        timed_out: false,
      },
    },
    {
      strategy: "clustered",
      best: false,
      route: ["A", "B", "C"],
      feasible: true,
      objective_score: {
        latest_start: "08:50",
        finish_time: "12:20",
        travel_minutes: 75,
        wait_minutes: 5,
      },
      elapsed_ms: 4,
      metadata: {
        state_count: 14,
        frontier_state_count: 7,
        cluster_count: 2,
        cluster_sizes: [1, 1],
        cluster_strategy: "directed_nearest_neighbor",
        cluster_order_strategy: "greedy_bridge",
        cluster_order: [2, 1],
        cluster_details: [
          {
            cluster: 2,
            members: ["B"],
            route: ["B"],
            entry: "B",
            exit: "B",
            state_count: 8,
            frontier_state_count: 4,
          },
          {
            cluster: 1,
            members: ["C"],
            route: ["C"],
            entry: "C",
            exit: "C",
            state_count: 6,
            frontier_state_count: 3,
          },
        ],
        score_before_improvement: 84,
        score_after_improvement: 80,
        improvement_strategy: "boundary_swap",
        swap_enabled: true,
        relocate_enabled: false,
        two_opt_enabled: false,
        improved_moves: 1,
        timed_out: false,
      },
    },
    {
      strategy: "mst_double_tree",
      best: false,
      route: ["A", "B", "C"],
      feasible: true,
      objective_score: {
        latest_start: "08:40",
        finish_time: "12:25",
        travel_minutes: 80,
        wait_minutes: 5,
      },
      elapsed_ms: 2,
      metadata: {
        symmetric_distance_strategy: "average_bidirectional",
        mst_cost: 59,
        mst_edge_count: 2,
        mst_edges: [
          { from: "A", to: "B", distance: 29 },
          { from: "B", to: "C", distance: 30 },
        ],
        euler_tour: ["A", "B", "C", "B", "A"],
        shortcut_route: ["A", "B", "C"],
        seed: 0,
        timed_out: false,
      },
    },
    {
      strategy: "christofides",
      best: false,
      route: ["A", "B", "C"],
      feasible: true,
      objective_score: {
        latest_start: "08:50",
        finish_time: "12:18",
        travel_minutes: 73,
        wait_minutes: 2,
      },
      elapsed_ms: 3,
      metadata: {
        symmetric_distance_strategy: "average_bidirectional",
        mst_cost: 59,
        mst_edge_count: 2,
        mst_edges: [
          { from: "A", to: "B", distance: 29 },
          { from: "B", to: "C", distance: 30 },
        ],
        odd_vertices: ["A", "C"],
        odd_vertex_count: 2,
        matching_strategy: "bit_dp",
        matching_cost: 27,
        matching_pairs: [{ left: "A", right: "C", distance: 27 }],
        euler_tour: ["A", "B", "C", "A"],
        shortcut_route: ["A", "B", "C"],
        seed: 0,
        timed_out: false,
      },
    },
    successfulSaCandidate("sa_greedy_seed_42", "greedy"),
    successfulSaCandidate("sa_christofides_seed_42", "christofides"),
    successfulSaCandidate("sa_clustered_seed_42", "clustered"),
    {
      strategy: "sa_mst_seed_42",
      best: false,
      route: [],
      feasible: false,
      elapsed_ms: 30,
      metadata: {
        seed: 42,
        timed_out: true,
        error: "strategy timed out",
      },
    },
  ],
};

test.beforeEach(async ({ page }) => {
  await page.route("**/api/health", (route) =>
    route.fulfill({ json: { status: "ok" } }),
  );
  await page.route("**/api/integration/jobs?limit=50", (route) =>
    route.fulfill({ json: { jobs: [] } }),
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
    const request = route.request().postDataJSON();
    expect(request.job_id).toBe("route-immediate");
    expect(request.start_time).toBe("00:00");
    expect(request.travel_time_matrix).toEqual([
      [0, 30, 45],
      [28, 0, 15],
      [40, 18, 0],
    ]);
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
  await expect(page.getByRole("table").first()).toContainText("09:25");
  await page.getByRole("button", { name: "알고리즘 비교" }).click();
  const benchmark = page.getByRole("dialog", { name: "Solver Benchmark" });
  await expect(benchmark.getByRole("heading", { name: "Overview" })).toBeVisible();
  const strategyTable = benchmark.getByRole("table");
  await expect(strategyTable).toContainText("exact_bit_dp");
  await expect(strategyTable).toContainText("★ Best");
  await expect(strategyTable).toContainText("sa_mst_seed_42");
  await expect(benchmark.getByRole("heading", { name: "Runtime 비교" })).toBeVisible();
  await expect(benchmark.getByLabel("exact_bit_dp runtime 7 ms")).toBeVisible();

  const navigation = benchmark.getByRole("navigation", { name: "Solver Benchmark 탐색" });
  for (const name of [
    "Overview",
    "Exact Bit DP",
    "Clustered",
    "MST Double-Tree",
    "Christofides",
    "SA (Greedy)",
    "SA (MST)",
    "SA (Christofides)",
    "SA (Clustered)",
  ]) {
    await expect(navigation.getByRole("button", { name: new RegExp(`^${name.replace(/[()]/g, "\\$&")}`) })).toBeVisible();
  }

  await navigation.getByRole("button", { name: /^Exact Bit DP/ }).click();
  await expect(benchmark.getByRole("heading", { name: "Exact Bit DP" })).toBeVisible();
  await expect(benchmark.getByText("Yes · ★ Best", { exact: true })).toBeVisible();
  await expect(benchmark.getByRole("heading", { name: "Summary" })).toBeVisible();
  await expect(benchmark.getByRole("heading", { name: "DP Statistics" })).toBeVisible();
  await expect(benchmark.getByText("Generated States", { exact: true })).toBeVisible();
  await expect(benchmark.getByText("Frontier States", { exact: true })).toBeVisible();
  await expect(benchmark.getByText("Frontier Cells", { exact: true })).toBeVisible();
  await expect(benchmark.getByText("55.6%", { exact: true })).toBeVisible();
  await expect(benchmark.getByText("Location Count", { exact: true })).toBeVisible();
  await expect(benchmark.getByText("Time Slot Size", { exact: true })).toBeVisible();
  await expect(benchmark.getByText("10 min", { exact: true })).toBeVisible();
  await expect(benchmark.getByLabel("Exact Bit DP Route")).toHaveText("A → B → C");
  await expect(benchmark.getByRole("heading", { name: "Objective" })).toBeVisible();
  await benchmark.getByRole("button", { name: "Raw Diagnostics" }).click();
  await expect(benchmark.getByLabel("Exact Bit DP Raw Diagnostics")).toContainText('"state_count": 18');
  await expect(benchmark.getByLabel("Exact Bit DP Raw Diagnostics")).toContainText('"frontier_cell_count": 6');

  await navigation.getByRole("button", { name: /^SA \(Greedy\)/ }).click();
  await expect(benchmark.getByRole("heading", { name: "SA (Greedy)" })).toBeVisible();
  await expect(benchmark.getByRole("heading", { name: "Summary" })).toBeVisible();
  await expect(benchmark.getByText("Initial Strategy", { exact: true })).toBeVisible();
  await expect(benchmark.getByText("greedy", { exact: true })).toBeVisible();
  await expect(benchmark.getByRole("heading", { name: "Search Statistics" })).toBeVisible();
  await expect(benchmark.getByText("25.0%", { exact: true })).toBeVisible();
  await expect(benchmark.getByText("Initial Temperature", { exact: true })).toBeVisible();
  await expect(benchmark.getByRole("heading", { name: "Score Improvement" })).toBeVisible();
  await expect(benchmark.getByText("Improvement", { exact: true })).toBeVisible();
  await expect(benchmark.getByLabel("SA (Greedy) Initial Route")).toHaveText("A → B → C");
  await expect(benchmark.getByLabel("SA (Greedy) Final Route")).toHaveText("A → B → C");
  await expect(benchmark.getByRole("heading", { name: "Move Statistics" })).toBeVisible();
  await expect(benchmark.getByText("Swap Attempts", { exact: true })).toBeVisible();
  await expect(benchmark.getByText("Best Feasible Solution: Yes", { exact: true })).toBeVisible();
  await benchmark.getByRole("button", { name: "Raw Diagnostics" }).click();
  await expect(benchmark.getByLabel("SA (Greedy) Raw Diagnostics")).toContainText('"iteration_count": 100');

  await navigation.getByRole("button", { name: /^SA \(MST\)/ }).click();
  await expect(benchmark.getByRole("heading", { name: "SA (MST)" })).toBeVisible();
  await expect(benchmark.getByRole("code")).toHaveText("sa_mst_seed_42");
  await expect(benchmark.getByText("Best Feasible Solution: No", { exact: true })).toBeVisible();
  await benchmark.getByRole("button", { name: "Raw Diagnostics" }).click();
  await expect(benchmark.getByLabel("SA (MST) Raw Diagnostics")).toContainText("strategy timed out");

  await navigation.getByRole("button", { name: /^Clustered/ }).click();
  await expect(benchmark.getByRole("heading", { name: "Clustered" })).toBeVisible();
  await expect(benchmark.getByRole("heading", { name: "Summary" })).toBeVisible();
  await expect(benchmark.getByRole("heading", { name: "Cluster Overview" })).toBeVisible();
  await expect(benchmark.getByText("Cluster Count", { exact: true })).toBeVisible();
  await expect(benchmark.getByText("1 / 1", { exact: true })).toBeVisible();
  await expect(benchmark.getByText("directed_nearest_neighbor", { exact: true })).toBeVisible();
  await expect(benchmark.getByLabel("Cluster별 장소 목록")).toContainText("Cluster 1");
  await expect(benchmark.getByLabel("Cluster Order")).toHaveText("Cluster 2 → Cluster 1");
  await expect(benchmark.getByRole("heading", { name: "Cluster Detail" })).toBeVisible();
  await expect(benchmark.getByText("Exact Generated", { exact: true }).first()).toBeVisible();
  await expect(benchmark.getByRole("heading", { name: "Merge / Improvement" })).toBeVisible();
  await expect(benchmark.getByText("Before Score", { exact: true })).toBeVisible();
  await expect(benchmark.getByText("boundary_swap", { exact: true })).toBeVisible();
  await expect(benchmark.getByText("Swap", { exact: true })).toBeVisible();
  await expect(benchmark.getByLabel("Clustered Final Route")).toHaveText("A → B → C");
  await benchmark.getByRole("button", { name: "Raw Diagnostics" }).click();
  await expect(benchmark.getByLabel("Clustered Raw Diagnostics")).toContainText('"cluster_count": 2');
  await expect(benchmark.getByLabel("Clustered Raw Diagnostics")).toContainText('"score_after_improvement": 80');

  await navigation.getByRole("button", { name: /^MST Double-Tree/ }).click();
  await expect(benchmark.getByRole("heading", { name: "MST Double-Tree" })).toBeVisible();
  await expect(benchmark.getByRole("heading", { name: "Summary" })).toBeVisible();
  await expect(benchmark.getByRole("heading", { name: "MST", exact: true })).toBeVisible();
  await expect(benchmark.getByText("average_bidirectional", { exact: true })).toBeVisible();
  await expect(benchmark.getByText("MST Cost", { exact: true })).toBeVisible();
  await expect(benchmark.getByText("MST Edge Count", { exact: true })).toBeVisible();
  await expect(benchmark.getByRole("table", { name: "MST Edges" })).toContainText("A");
  await expect(benchmark.getByLabel("MST Euler Tour")).toHaveText("A → B → C → B → A");
  await expect(benchmark.getByLabel("MST Shortcut Result")).toHaveText("A → B → C");
  await expect(benchmark.getByRole("heading", { name: "Original Directed Matrix Evaluation" })).toBeVisible();
  await expect(benchmark.getByText("2-approx 보장은 symmetric metric TSP에서만 성립합니다.")).toBeVisible();
  await benchmark.getByRole("button", { name: "Raw Diagnostics" }).click();
  await expect(benchmark.getByLabel("MST Double-Tree Raw Diagnostics")).toContainText('"mst_cost": 59');
  await expect(benchmark.getByLabel("MST Double-Tree Raw Diagnostics")).toContainText('"euler_tour"');

  await navigation.getByRole("button", { name: /^Christofides/ }).click();
  await expect(benchmark.getByRole("heading", { name: "Christofides" })).toBeVisible();
  await expect(benchmark.getByRole("heading", { name: "Summary" })).toBeVisible();
  await expect(benchmark.getByRole("heading", { name: "MST", exact: true })).toBeVisible();
  await expect(benchmark.getByRole("table", { name: "Christofides MST Edges" })).toContainText("29");
  await expect(benchmark.getByRole("heading", { name: "Odd Vertices" })).toBeVisible();
  await expect(benchmark.getByText("Odd Vertex Count", { exact: true })).toBeVisible();
  await expect(benchmark.getByLabel("Christofides Odd Vertices")).toHaveText("A · C");
  await expect(benchmark.getByRole("heading", { name: "Perfect Matching" })).toBeVisible();
  await expect(benchmark.getByText("BitDP", { exact: true })).toBeVisible();
  await expect(benchmark.getByText("Matching Cost", { exact: true })).toBeVisible();
  await expect(benchmark.getByRole("table", { name: "Christofides Matching Pairs" })).toContainText("27");
  await expect(benchmark.getByLabel("Christofides Euler Tour")).toHaveText("A → B → C → A");
  await expect(benchmark.getByLabel("Christofides Shortcut Route")).toHaveText("A → B → C");
  await expect(benchmark.getByRole("heading", { name: "Original Directed Matrix Evaluation" })).toBeVisible();
  await expect(benchmark.getByText("1.5-approx 보장은 symmetric metric TSP에서만 성립합니다.")).toBeVisible();
  await benchmark.getByRole("button", { name: "Raw Diagnostics" }).click();
  await expect(benchmark.getByLabel("Christofides Raw Diagnostics")).toContainText('"matching_strategy": "bit_dp"');
  await expect(benchmark.getByLabel("Christofides Raw Diagnostics")).toContainText('"odd_vertices"');
  for (const name of [
    "SA (Christofides)",
    "SA (Clustered)",
  ]) {
    const escaped = name.replace(/[()]/g, "\\$&");
    await navigation.getByRole("button", { name: new RegExp(`^${escaped}`) }).click();
    await expect(benchmark.getByRole("heading", { name })).toBeVisible();
    await expect(benchmark.getByRole("heading", { name: "Search Statistics" })).toBeVisible();
    await expect(benchmark.getByRole("heading", { name: "Route Comparison" })).toBeVisible();
  }
  await benchmark.getByRole("button", { name: "Close" }).click();
  await expect(page.getByText("총 이동 시간", { exact: true })).toBeVisible();
  await expect(page.locator(".route-overview strong")).toHaveText("70분");
  await expect(page.getByText("HTTP 200", { exact: true })).toBeVisible();
  await expect(page.getByText(/\d+\.\d ms/)).toBeVisible();
  await expect(page.getByLabel("Raw API 응답")).toContainText(
    "total_travel_minutes",
  );
  await page.getByRole("button", { name: "복사", exact: true }).click();
  await expect(page.getByRole("button", { name: "복사됨" })).toBeVisible();
});

test("form edits directed matrix, resizes with locations, and exposes five presets", async ({
  page,
}) => {
  let presetMatrixRequests = 0;
  await page.route("**/api/integration/matrix", async (route) => {
    presetMatrixRequests += 1;
    const request = route.request().postDataJSON();
    const size = request.locations.length;
    const matrix = Array.from({ length: size }, (_, row) =>
      Array.from({ length: size }, (_, column) =>
        row === column ? 0 : (row + 1) * 10 + column + 1,
      ),
    );
    await route.fulfill({ json: { travel_time_matrix: matrix } });
  });
  await page.goto("/");
  const { dialog } = await openJobDialog(page);

  await expect(dialog.getByRole("heading", { name: "기본 정보" })).toBeVisible();
  await expect(dialog.getByText("10분", { exact: true })).toBeVisible();
  await expect(dialog.getByLabel("A에서 B 이동 시간")).toHaveValue("30");
  await expect(dialog.getByLabel("B에서 A 이동 시간")).toHaveValue("28");
  await dialog.getByLabel("A에서 B 이동 시간").fill("37");
  await expect(dialog.getByLabel("B에서 A 이동 시간")).toHaveValue("28");

  await dialog.getByRole("button", { name: "장소 추가" }).click();
  await expect(dialog.locator(".location-input-table tbody tr")).toHaveCount(4);
  await expect(dialog.locator(".matrix-input-table tbody input")).toHaveCount(16);
  await dialog.getByRole("button", { name: "D 장소 삭제" }).click();
  await expect(dialog.locator(".matrix-input-table tbody input")).toHaveCount(9);

  for (const name of ["Tokyo 3", "Tokyo 5", "Tokyo 10", "Seoul 5", "Synthetic 8"]) {
    await expect(dialog.getByRole("button", { name, exact: true })).toBeVisible();
  }
  await expect(dialog.getByRole("button", { name: "Seoul 3", exact: true })).toHaveCount(0);

  await dialog.getByRole("button", { name: "Tokyo 3", exact: true }).click();
  await expect(page.getByText("현재 입력을 Tokyo 3 preset으로 교체할까요?")).toBeVisible();
  await page.getByRole("button", { name: "취소", exact: true }).last().click();
  await expect(dialog.getByLabel("A에서 B 이동 시간")).toHaveValue("37");
  expect(presetMatrixRequests).toBe(0);

  await dialog.getByRole("button", { name: "Tokyo 5", exact: true }).click();
  await expect(page.getByText("현재 입력을 Tokyo 5 preset으로 교체할까요?")).toBeVisible();
  await page.getByRole("button", { name: "교체", exact: true }).click();
  await expect.poll(() => presetMatrixRequests).toBe(1);
  await expect(dialog.locator(".location-input-table tbody tr")).toHaveCount(5);
  await expect(dialog.getByLabel("1번 장소 Place ID")).not.toHaveValue("");
  await expect(dialog.locator(".matrix-input-table tbody input")).toHaveCount(25);
  await expect(dialog.getByLabel("tokyo-station에서 shibuya-station 이동 시간")).toHaveValue("12");
  await dialog.getByRole("button", { name: "검증" }).click();
  await expect(dialog.getByText("유효함", { exact: true })).toBeVisible();

  await dialog.getByRole("button", { name: "Synthetic 8", exact: true }).click();
  await expect(page.getByText("현재 입력을 Synthetic 8 preset으로 교체할까요?")).toBeVisible();
  await page.getByRole("button", { name: "교체", exact: true }).click();
  await expect(dialog.locator(".location-input-table tbody tr")).toHaveCount(8);
  await expect(dialog.getByLabel("1번 장소 Place ID")).toHaveValue("");
  await expect(dialog.locator(".matrix-input-table tbody input")).toHaveCount(64);
  await expect(dialog.getByLabel("A에서 B 이동 시간")).toHaveValue("12");
  await expect(dialog.getByLabel("B에서 A 이동 시간")).toHaveValue("17");
  await dialog.getByRole("button", { name: "검증" }).click();
  await expect(dialog.getByText("유효함", { exact: true })).toBeVisible();
  expect(presetMatrixRequests).toBe(1);
});

test("CSV and tcache matrix input update the shared form and raw JSON", async ({
  page,
}) => {
  let matrixRequests = 0;
  await page.route("**/api/integration/matrix", async (route) => {
    matrixRequests += 1;
    const request = route.request().postDataJSON();
    expect(request.travel_time_matrix).toBeUndefined();
    await route.fulfill({
      json: { travel_time_matrix: [[0, 11, 12], [21, 0, 23], [31, 32, 0]] },
    });
  });
  await page.goto("/");
  const { dialog, editor } = await openJobDialog(page);

  await dialog.getByRole("button", { name: "CSV 붙여넣기" }).click();
  await dialog.getByLabel("매트릭스 CSV").fill("0,5,6\n7,0,8\n9,10,0");
  await dialog.getByRole("button", { name: "CSV 적용" }).click();
  await expect(dialog.getByLabel("A에서 B 이동 시간")).toHaveValue("5");
  await expect(editor).toHaveValue(/"travel_time_matrix"/);

  await dialog.getByRole("button", { name: "tcache에서 가져오기" }).click();
  await expect.poll(() => matrixRequests).toBe(1);
  await expect(dialog.getByLabel("A에서 B 이동 시간")).toHaveValue("11");
  await expect(dialog.getByLabel("B에서 A 이동 시간")).toHaveValue("21");
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

test("reload restores recent jobs and fetches the selected timeline", async ({
  page,
}) => {
  let timelineCalls = 0;
  await page.route("**/api/integration/jobs?limit=50", (route) =>
    route.fulfill({
      json: {
        jobs: [
          {
            job_id: "route-persisted",
            status: "completed",
            created_at: 1789521000000,
            updated_at: 1789521005000,
          },
        ],
      },
    }),
  );
  await page.route("**/api/integration/jobs/route-persisted/timeline", (route) => {
    timelineCalls += 1;
    return route.fulfill({
      json: {
        job_id: "route-persisted",
        entries: [
          {
            id: "evt-1",
            pair_id: "opt-1",
            timestamp_ms: 1789521000000,
            direction: "REQUEST",
            source: "testbed",
            target: "troute",
            method: "POST",
            path: "/optimize",
          },
        ],
      },
    });
  });
  await page.route("**/api/integration/jobs/route-persisted", (route) =>
    route.fulfill({
      json: {
        request: {
          job_id: "route-persisted",
          locations: [
            {
              id: "A",
              place_id: "place-a",
              open_time: "00:00",
              close_time: "23:59",
              stay_minutes: 0,
            },
            {
              id: "C",
              place_id: "place-c",
              open_time: "00:00",
              close_time: "23:59",
              stay_minutes: 0,
            },
          ],
          start_time: "09:00",
        },
        job_id: "route-persisted",
        status: "completed",
        stage: "scheduling",
        progress: 100,
        last_message: "Building itinerary schedule.",
        created_at: 1789521000000,
        updated_at: 1789521005000,
        completed_at: 1789521005000,
        result: routeResult,
        error: null,
      },
    }),
  );

  await page.goto("/");
  const restored = page.getByRole("option", { name: /route-persisted/ });
  await expect(restored).toBeVisible();
  await expect(restored).toHaveAttribute("aria-selected", "true");
  await expect(page.getByRole("heading", { name: "route-persisted" })).toBeVisible();
  await expect(page.getByLabel("방문 순서")).toHaveText("A → B → C");
  await expect.poll(() => timelineCalls).toBe(1);

  await page.reload();
  await expect(page.getByRole("option", { name: /route-persisted/ })).toBeVisible();
  await expect(page.getByRole("heading", { name: "route-persisted" })).toBeVisible();
  await expect.poll(() => timelineCalls).toBe(2);
});

test("running job can be force-cancelled and remains cancelled after optimize ends", async ({
  page,
}) => {
  let releaseOptimize!: () => void;
  const optimizeGate = new Promise<void>((resolve) => {
    releaseOptimize = resolve;
  });
  await page.route("**/api/fixture-route", async (route) => {
    await optimizeGate;
    await route.fulfill({
      status: 409,
      json: {
        error: {
          code: "JOB_CANCELLED",
          message: "The optimization job was cancelled.",
        },
      },
    });
  });
  await page.route("**/api/integration/jobs/route-cancel/cancel", (route) => {
    expect(route.request().method()).toBe("POST");
    return route.fulfill({
      json: { job_id: "route-cancel", status: "cancelled" },
    });
  });
  await page.route("**/api/integration/jobs/route-cancel/timeline", (route) =>
    route.fulfill({ json: { job_id: "route-cancel", entries: [] } }),
  );
  await page.goto("/");

  await createJob(page, "route-cancel");
  const row = page.getByRole("option", { name: /route-cancel/ });
  await expect(row).toHaveAttribute("data-status", "running");
  await page.getByRole("button", { name: "Job 강제 종료" }).click();
  const dialog = page.getByRole("dialog", { name: "Job 강제 종료" });
  await expect(dialog).toContainText("이미 수행된 기록은 유지");
  await dialog.getByRole("button", { name: "강제 종료" }).click();

  await expect(row).toHaveAttribute("data-status", "cancelled");
  await expect(row).toContainText("취소됨");
  await expect(page.getByText("취소됨", { exact: true }).last()).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Job 강제 종료" }),
  ).toHaveCount(0);

  releaseOptimize();
  await expect(row).toHaveAttribute("data-status", "cancelled");
  await expect(row).not.toContainText("오류");
});
