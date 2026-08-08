import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  test: { environment: "jsdom" },
  clearScreen: false,
  server: { strictPort: true, port: 1420 },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: { target: "safari13", minify: process.env.TAURI_ENV_DEBUG ? false : "esbuild" },
});
