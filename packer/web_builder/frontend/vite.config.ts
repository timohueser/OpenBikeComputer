import { svelte } from "@sveltejs/vite-plugin-svelte";
import { defineConfig } from "vite";

// base "./" keeps every asset URL relative, so the built app works mounted at
// "/" (local FastAPI) or under a sub-path (a future single-server deployment
// serving landing + docs + builder behind one reverse proxy).
export default defineConfig({
    base: "./",
    plugins: [svelte()],
    build: {
        outDir: "../static/dist",
        emptyOutDir: true,
    },
    server: {
        // Dev mode: `python -m packer.web_builder --no-browser` on :8000 serves
        // the API; Vite proxies it (plain http-proxy streams SSE fine).
        proxy: {
            "/api": "http://127.0.0.1:8000",
        },
    },
    test: {
        environment: "node",
        include: ["src/**/*.test.ts"],
    },
});
