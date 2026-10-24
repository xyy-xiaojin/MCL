import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";

import pkg from "./package.json";

// Tauri 会以 devUrl 指向固定的 1420 端口，端口被占用时必须直接失败，
// 否则 Tauri 窗口会加载到别的进程上。
export default defineConfig({
  plugins: [vue()],
  // 版本号只维护 package.json 一处，界面通过 __APP_VERSION__ 读
  define: { __APP_VERSION__: JSON.stringify(pkg.version) },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      // src-tauri 由 cargo 自己监听，Vite 不需要重复扫描。
      ignored: ["**/src-tauri/**"],
    },
  },
  build: {
    target: "chrome105",
    outDir: "dist",
  },
});
