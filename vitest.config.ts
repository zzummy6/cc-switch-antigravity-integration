import path from "node:path";
import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  test: {
    environment: "jsdom",
    setupFiles: ["./tests/setupGlobals.ts", "./tests/setupTests.ts"],
    globals: true,
    // tests/e2e 是 Playwright（浏览器级）专属，不能被 vitest 加载
    exclude: ["**/node_modules/**", "tests/e2e/**"],
    coverage: {
      reporter: ["text", "lcov"],
    },
  },
});
