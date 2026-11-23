/// <reference types="vitest/config" />
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// The browser only ever talks to /api on the Vite origin; Vite forwards it to
// the Rust backend, so the Anthropic key never reaches the frontend.
export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    port: 5173,
    proxy: {
      "/api": {
        target: process.env.VITE_BACKEND_URL ?? "http://localhost:8000",
        changeOrigin: true,
      },
    },
  },
  test: {
    // Component tests render into jsdom; the pure helpers in src/lib run there too.
    environment: "jsdom",
    setupFiles: ["src/test/setup.ts"],
  },
});
