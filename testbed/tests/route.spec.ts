import { test, expect } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  await page.route("**/api/health", (route) =>
    route.fulfill({ json: { status: "ok" } }),
  );
});

test("valid build commit renders a short linked SHA", async ({ page }) => {
  await page.goto("/");

  const commit = page.getByRole("link", {
    name: "Commit 3f4f8d25686fe955582295e1e47334c7a277c681",
  });
  await expect(commit).toContainText("commit 3f4f8d2");
  await expect(commit).toHaveAttribute(
    "href",
    "https://github.com/egod1537/troute/commit/3f4f8d25686fe955582295e1e47334c7a277c681",
  );
  await expect(commit).toHaveAttribute("target", "_blank");
  await expect(commit).toHaveAttribute("rel", "noreferrer");
  await expect(page.getByLabel("API Online")).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Refresh API health" }),
  ).toBeVisible();
});

test("configured client posts input and renders one inspectable response", async ({
  page,
}) => {
  await page.route("**/api/fixture-route", async (route) => {
    expect(route.request().method()).toBe("POST");
    expect(route.request().postDataJSON().job_id).toBe("route-testbed-sample");
    expect(route.request().postDataJSON().start_location_id).toBe("A");
    await route.fulfill({
      json: {
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
          { order: 2, location_id: "A", arrival_time: "12:15" },
        ],
        total_travel_minutes: 70,
      },
    });
  });

  await page.goto("/");
  await expect(page.getByLabel("API Online")).toBeVisible();
  await page.getByRole("button", { name: "Run", exact: true }).click();

  await expect(page.getByLabel("Visit order")).toHaveText("A → B → A");
  await expect(page.getByRole("table")).toContainText("09:25");
  await expect(page.getByRole("table")).toContainText("11:30");
  await expect(page.getByText("70 min", { exact: true })).toBeVisible();
  await expect(page.getByText("HTTP 200", { exact: true })).toBeVisible();
  await expect(page.getByText(/\d+\.\d ms/)).toBeVisible();
  await expect(page.getByLabel("Raw API response")).toContainText(
    "total_travel_minutes",
  );

  await page.getByRole("button", { name: "Copy", exact: true }).click();
  await expect(page.getByRole("button", { name: "Copied" })).toBeVisible();
});

test("invalid input never sends a route request", async ({ page }) => {
  let calls = 0;
  await page.route("**/api/fixture-route", (route) => {
    calls++;
    return route.fulfill({ json: {} });
  });
  await page.goto("/");
  await expect(page.getByLabel("API Online")).toBeVisible();

  await page.getByLabel("Request JSON").fill("{}");
  await page.getByRole("button", { name: "Run", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("Input requires");
  expect(calls).toBe(0);
});

for (const kind of ["http-error", "solver-error", "malformed"] as const) {
  test(`route ${kind} remains inspectable`, async ({ page }) => {
    await page.route("**/api/fixture-route", (route) =>
      route.fulfill({
        status: kind === "http-error" ? 422 : 200,
        json:
          kind === "malformed"
            ? { route: "incorrect shape" }
            : { error: "infeasible route: visit window exceeded" },
      }),
    );
    await page.goto("/");
    await expect(page.getByLabel("API Online")).toBeVisible();

    await page.getByRole("button", { name: "Run", exact: true }).click();
    await expect(page.getByRole("alert")).toBeVisible();
    await expect(page.getByLabel("Raw API response")).toContainText(
      kind === "malformed" ? "incorrect shape" : "infeasible route",
    );
    await expect(page.getByRole("table")).toHaveCount(0);
    if (kind === "http-error") {
      await expect(page.getByText("HTTP 422", { exact: true })).toBeVisible();
      await expect(page.getByText(/\d+\.\d ms/)).toBeVisible();
    }
  });
}
