import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: "./tests",
  fullyParallel: true,
  use: { ...devices["Desktop Chrome"], trace: "retain-on-failure" },
  projects: [
    {
      name: "health-only",
      testMatch: "health.spec.ts",
      use: { baseURL: "http://127.0.0.1:4173" },
    },
    {
      name: "route-client",
      testMatch: "route.spec.ts",
      use: { baseURL: "http://127.0.0.1:4174" },
    },
  ],
  webServer: [
    {
      command: "npm run dev -- --port 4173",
      url: "http://127.0.0.1:4173",
      env: { VITE_TROUTE_ROUTE_PATH: "", VITE_TROUTE_API_BASE_URL: "/api" },
    },
    // This path is a browser-test fixture, never an implemented Rust endpoint.
    {
      command: "npm run dev -- --port 4174",
      url: "http://127.0.0.1:4174",
      env: {
        VITE_TROUTE_ROUTE_PATH: "/fixture-route",
        VITE_TROUTE_API_BASE_URL: "/api",
      },
    },
  ],
});
