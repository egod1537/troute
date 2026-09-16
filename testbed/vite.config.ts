import { readFileSync } from "node:fs";
import { defineConfig, loadEnv } from "vite";
import react from "@vitejs/plugin-react";

const canonicalIcon = readFileSync(
  new URL("../assets/troute-icon.svg", import.meta.url),
  "utf8",
);
const favicon = readFileSync(
  new URL("./public/troute-icon.svg", import.meta.url),
  "utf8",
);
if (favicon !== canonicalIcon.replaceAll("currentColor", "#2D72D2")) {
  throw new Error(
    "testbed/public/troute-icon.svg must mirror assets/troute-icon.svg with an explicit favicon color.",
  );
}

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, process.cwd(), "");
  return {
    plugins: [react()],
    server: {
      strictPort: true,
      proxy: {
        "/api": {
          target: env.TROUTE_DEV_PROXY_TARGET || "http://127.0.0.1:8080",
          changeOrigin: true,
          rewrite: (path) => path.replace(/^\/api/, ""),
        },
      },
    },
  };
});
