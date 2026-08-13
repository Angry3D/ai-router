import { resolve } from "node:path";

import react from "@vitejs/plugin-react";
import { configDefaults, defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    host: "127.0.0.1",
    port: 1420,
    strictPort: true,
  },
  build: {
    target: "safari13",
    rollupOptions: {
      input: {
        menu: resolve(import.meta.dirname, "menu.html"),
        settings: resolve(import.meta.dirname, "settings.html"),
      },
    },
  },
  test: {
    environment: "jsdom",
    exclude: [...configDefaults.exclude, ".trellis/**"],
    setupFiles: "./vitest.setup.ts",
  },
});
