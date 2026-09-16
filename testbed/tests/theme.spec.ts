import { expect, test, type Page } from "@playwright/test";

const STORAGE_KEY = "troute.testbed.theme";

test.beforeEach(async ({ page }) => {
  await page.route("**/api/health", (route) =>
    route.fulfill({ json: { status: "ok" } }),
  );
});

async function storeTheme(page: Page, value: string) {
  await page.addInitScript(
    ({ key, stored }) => window.localStorage.setItem(key, stored),
    { key: STORAGE_KEY, stored: value },
  );
}

function appShell(page: Page) {
  return page.locator(".app-shell");
}

test("defaults to system mode and resolves the current system preference", async ({
  page,
}) => {
  await page.emulateMedia({ colorScheme: "dark" });
  await page.goto("/");

  await expect(page.getByRole("button", { name: "Theme: System" })).toBeVisible();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await expect(appShell(page)).toHaveAttribute("data-theme", "dark");
  await expect(appShell(page)).toHaveClass(/bp6-dark/);
  expect(await page.evaluate((key) => localStorage.getItem(key), STORAGE_KEY)).toBeNull();
});

test("restores saved light mode independently of a dark system", async ({ page }) => {
  await page.emulateMedia({ colorScheme: "dark" });
  await storeTheme(page, "light");
  await page.goto("/");

  await expect(page.getByRole("button", { name: "Theme: Light" })).toBeVisible();
  await expect(appShell(page)).toHaveAttribute("data-theme", "light");
  await expect(appShell(page)).not.toHaveClass(/bp6-dark/);
});

test("restores saved dark mode independently of a light system", async ({ page }) => {
  await page.emulateMedia({ colorScheme: "light" });
  await storeTheme(page, "dark");
  await page.goto("/");

  await expect(page.getByRole("button", { name: "Theme: Dark" })).toBeVisible();
  await expect(appShell(page)).toHaveAttribute("data-theme", "dark");
  await expect(appShell(page)).toHaveClass(/bp6-dark/);

  await page.getByRole("button", { name: "New Job" }).click();
  await expect(page.locator(".bp6-portal:has(.new-job-dialog)")).toHaveClass(
    /bp6-dark/,
  );
  await expect(page.getByRole("dialog", { name: "New Job" })).toBeVisible();
});

test("invalid saved values fall back to system mode", async ({ page }) => {
  await page.emulateMedia({ colorScheme: "dark" });
  await storeTheme(page, "sepia");
  await page.goto("/");

  await expect(page.getByRole("button", { name: "Theme: System" })).toBeVisible();
  await expect(appShell(page)).toHaveAttribute("data-theme", "dark");
  await expect(appShell(page)).toHaveClass(/bp6-dark/);
});

test("theme control switches modes, stores the choice, and restores it", async ({
  page,
}) => {
  await page.emulateMedia({ colorScheme: "light" });
  await page.goto("/");

  await page.getByRole("button", { name: "Theme: System" }).click();
  await page.getByRole("menuitem", { name: "Dark" }).click();
  await expect(appShell(page)).toHaveClass(/bp6-dark/);
  expect(
    await page.evaluate((key) => localStorage.getItem(key), STORAGE_KEY),
  ).toBe("dark");

  await page.reload();
  await expect(page.getByRole("button", { name: "Theme: Dark" })).toBeVisible();
  await expect(appShell(page)).toHaveClass(/bp6-dark/);

  await page.getByRole("button", { name: "Theme: Dark" }).click();
  await page.getByRole("menuitem", { name: "Light" }).click();
  await expect(appShell(page)).not.toHaveClass(/bp6-dark/);
  await expect(appShell(page)).toHaveAttribute("data-theme", "light");

  await page.getByRole("button", { name: "Theme: Light" }).click();
  await page.getByRole("menuitem", { name: "System" }).click();
  await expect(appShell(page)).not.toHaveClass(/bp6-dark/);
  expect(
    await page.evaluate((key) => localStorage.getItem(key), STORAGE_KEY),
  ).toBe("system");
});

test("system mode reacts to system preference changes", async ({ page }) => {
  await page.emulateMedia({ colorScheme: "light" });
  await page.goto("/");
  await expect(appShell(page)).not.toHaveClass(/bp6-dark/);

  await page.emulateMedia({ colorScheme: "dark" });
  await expect(appShell(page)).toHaveClass(/bp6-dark/);
  await expect(appShell(page)).toHaveAttribute("data-theme", "dark");

  await page.emulateMedia({ colorScheme: "light" });
  await expect(appShell(page)).not.toHaveClass(/bp6-dark/);
  await expect(appShell(page)).toHaveAttribute("data-theme", "light");
});
