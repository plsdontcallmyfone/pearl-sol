import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { resolve } from "node:path";

export default defineConfig({
  plugins: [react()],
  base: "",
  build: {
    outDir: "dist",
    emptyOutDir: false,
    target: "chrome120",
    modulePreload: false,
    rollupOptions: { input: { popup: resolve(__dirname, "popup.html"), approval: resolve(__dirname, "approval.html") } },
  },
  test: { include: ["test/**/*.test.ts"], testTimeout: 20000 },
});
