import { defineConfig } from "vite";

// Tauri 期望固定端口；失败则报错而不是切换端口
export default defineConfig({
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    rollupOptions: { input: { main: "index.html", overlay: "overlay.html" } },
    target: "chrome105",
    minify: "esbuild",
    sourcemap: false,
  },
});
