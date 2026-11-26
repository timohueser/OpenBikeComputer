import { availableParallelism, loadavg } from "node:os";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { defineConfig, loadEnv } from "vite";
import { bundleVersions } from "./vite/bundle-version";
import { modelFiles } from "./vite/model-files";
import { stampVersion } from "./vite/stamp-bundle";
import { thirdPartyLicenses } from "./vite/third-party-licenses";
import { plannerConfig } from "./src/lib/planner/config";
import { testConfig } from "./test-support/planner/config";

// Modes that build one page of their own, into `dist/MODE`: the product, the operator page
// of `obc data ui` and the TrailView 3D view.
const PAGES: Record<string, string> = { planner: "planner.html", data: "data.html", trailview: "trailview.html" };

// `vite` adds the mode development and vitest the mode test. Anything else is a typo, and
// quietly building a mistyped deploy target is the one outcome worth failing over.
function checkMode(mode: string): void {
    if (mode in PAGES || mode === "development" || mode === "test") return;
    throw new Error(`unknown --mode "${mode}": expected one of ${Object.keys(PAGES).join(", ")} (or development/test)`);
}

export default defineConfig(({ mode }) => {
    checkMode(mode);
    // A preview config that the planner cannot use fails the build. Without one, the planner reads the live catalogue.
    const preview = loadEnv(mode, process.cwd(), "VITE_PLANNER_CONFIG").VITE_PLANNER_CONFIG;
    if ((mode === "planner" || mode === "trailview") && preview) plannerConfig(JSON.parse(preview));
    // `vite --mode data` sends the page's `/api/cli` to `obc data ui`. It keeps the Host:
    // `obc data ui` refuses an Origin that is not `http://` and the Host.
    const proxy = mode === "data" ? { "/api": { target: "http://127.0.0.1:8700", changeOrigin: false } } : undefined;
    return {
        // base "./" keeps every asset URL relative, so a built page works under any sub-path.
        base: "./",
        // The licence notices ride along with the bundle, which is a distribution.
        plugins: [svelte(), thirdPartyLicenses(), stampVersion(), bundleVersions(), modelFiles()],
        // Always a string, so a build without it drops the pretend device of `planner.ts` with its wasm.
        define: { "import.meta.env.VITE_FAKE_DEVICE": JSON.stringify(process.env.VITE_FAKE_DEVICE ?? "") },
        resolve: {
            // Component tests run through Vitest's SSR transform even when their
            // per-file environment supplies a DOM. Select Svelte's client runtime
            // there so mounted tests exercise the real browser lifecycle.
            ...(mode === "test" ? { conditions: ["browser"] } : {}),
        },
        build: {
            outDir: `dist/${mode}`,
            copyPublicDir: false,
            rollupOptions: { input: PAGES[mode] },
            emptyOutDir: true,
        },
        worker: {
                // The assembly worker dynamically imports the wasm bridge, so its bundle
                // code-splits — and rollup only code-splits ES output, while Vite's default
                // worker format is still "iife".
                //
                // Module workers are Chrome 80, Safari 15, Firefox 114 — comfortably below
                // what the app already needs elsewhere. A browser under that floor loses the
                // in-browser assembly, not the site: the worker only spawns when a map is
                // built.
            format: "es",
            // Workers bundle their imports with plugins of their own.
            plugins: () => [stampVersion()],
        },
        server: {
            // The product skin editor imports the bakery's canonical Teningen
            // OBCM from `host/obc-bake/assets`. Keep one fixture, and let the
            // dev server expose files only as far as this repository root.
            fs: { allow: ["../.."] },
            proxy,
        },
        test: {
            environment: "node",
            setupFiles: ["./test-support/core.ts"],
            // Planner modules read their config on import, as in a planner build.
            env: { VITE_PLANNER_CONFIG: JSON.stringify(testConfig), VITE_PLANNER_CATALOG: "" },
            // `test-support/` holds what no tier's build has as an input; its suites run here.
            include: ["src/**/*.test.ts", "test-support/**/*.test.ts"],
            // On a shared machine, only the cores that are idle when the run starts: a worker past
            // them slows every test, and the heavy ones past their limits. A CI runner is the job's own,
            // and its load is the job's earlier steps.
            maxWorkers: process.env.CI
                ? undefined
                : Math.max(1, Math.min(availableParallelism() - 1, Math.floor(availableParallelism() - loadavg()[0]))),
        },
    };
});
