import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri 期望固定端口;关闭 clearScreen 以免吞掉 vite 的网络日志。
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] },
  },
});
