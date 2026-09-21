import { svelte } from "@sveltejs/vite-plugin-svelte";
import { defineConfig } from "vite";
import { thirdPartyLicenses } from "./vite/third-party-licenses";

// One frontend, three hosts. Which host `$host` resolves to is decided here, at
// build time — a conditional alias, not a runtime `if` — so the two hosts you did not
// build have no path into the module graph at all. That is what keeps the FastAPI
// job-polling client out of the static web bundle, rather than trusting a bundler to
// notice that a branch is unreachable. src/lib/platform/bundle.test.ts asserts it
// against the real emitted chunks.
const HOSTS = {
    // The local FastAPI server, and the only one `python -m builder.server`
    // serves — hence the outDir it already mounts.
    dev: { module: "dev", outDir: "../server/static/dist" },
    web: { module: "web", outDir: "dist/web" },
    desktop: { module: "desktop", outDir: "dist/desktop" },
} as const;

type HostName = keyof typeof HOSTS;

// `vite`/`vite build` default to development/production, and vitest to test — all
// three are the dev host. `--mode web|desktop` opts into the others. Anything else is
// a typo, and quietly building the dev host for a mistyped deploy target is the one
// outcome worth failing over.
function hostFor(mode: string): HostName {
    if (mode in HOSTS) return mode as HostName;
    if (mode === "development" || mode === "production" || mode === "test") return "dev";
    throw new Error(
        `unknown --mode "${mode}": expected one of ${Object.keys(HOSTS).join(", ")} ` +
            "(or development/production/test, which build the dev host)",
    );
}

export default defineConfig(({ mode }) => {
    const host = HOSTS[hostFor(mode)];
    return {
        // base "./" keeps every asset URL relative, so the built app works mounted at
        // "/" (local FastAPI) or under a sub-path (a future single-server deployment
        // serving landing + docs + builder behind one reverse proxy).
        base: "./",
            // The licence notices ride along with every tier's bundle — the static site and
            // the Tauri app are the same build, and both are distributions.
        plugins: [svelte(), thirdPartyLicenses()],
        resolve: {
            // Root-relative rather than an absolute path so the config needs no
            // node: builtins (and so no @types/node just to type-check itself).
            alias: { $host: `/src/lib/platform/${host.module}.ts` },
            // Component tests run through Vitest's SSR transform even when their
            // per-file environment supplies a DOM. Select Svelte's client runtime
            // there so mounted tests exercise the real browser lifecycle.
            ...(mode === "test" ? { conditions: ["browser"] } : {}),
        },
        build: {
            outDir: host.outDir,
            emptyOutDir: true,
        },
        worker: {
                // The assembly worker dynamically imports the wasm bridge, so its bundle
                // code-splits — and rollup only code-splits ES output, while Vite's default
                // worker format is still "iife".
                //
                // Module workers are Chrome 80, Safari 15, Firefox 114 — comfortably below
                // what the app already needs elsewhere. A browser under that floor loses the
                // in-browser assembly, not the site: the worker only spawns from the
                // download step.
            format: "es",
        },
        server: {
            // The product skin editor imports the bakery's canonical Teningen
            // OBCM from `host/obc-bake/assets`. Keep one fixture, and let the
            // dev server expose files only as far as this repository root.
            fs: { allow: ["../.."] },
            // Dev mode: `python -m builder.server --no-browser` on :8000 serves
            // the API; Vite proxies it (plain http-proxy streams SSE fine).
            proxy: {
                "/api": `http://127.0.0.1:${process.env.OBC_BUILDER_PORT || "8000"}`,
            },
        },
        test: {
            environment: "node",
            // `test-support/` holds what no tier's build has as an input; its suites run here.
            include: ["src/**/*.test.ts", "test-support/**/*.test.ts"],
            coverage: {
                provider: "v8",
                include: ["src/**/*.{ts,js,svelte}"],
                exclude: ["src/**/*.test.ts", "src/lib/*/pkg/**"],
                reporter: ["text", "lcov", "json"],
                reportsDirectory: "../../.artifacts/coverage/web",
                reportOnFailure: true,
            },
        },
    };
});
