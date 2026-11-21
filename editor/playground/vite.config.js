import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";

const page = (name) => fileURLToPath(new URL(name, import.meta.url));

export default defineConfig({
  build: {
    target: "es2022",
    rollupOptions: { input: [page("index.html"), page("react.html"), page("vue.html")] },
  },
  worker: { format: "es" },
});
