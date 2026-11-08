// `virtual:plan-version`: the version of the JavaScript that prepares a ground plan cell's inputs (the plan
// worker, with the page decode that it imports, and the input walk). The device cache keys the cells by it and
// by the hash of the bridge's wasm, which the bundle only names by URL.

import { createHash } from "node:crypto";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { build, type Plugin } from "vite";

const ROOT = fileURLToPath(new URL("..", import.meta.url));
const ENTRIES = ["src/lib/trailview/plan/plan-worker.ts", "src/lib/trailview/plan/inputs.ts"];

/** The start of the SHA-256 of the entries bundled with every import inlined, and the files of the modules in them. */
async function planBundle(): Promise<{ version: string; modules: string[] }> {
    const hash = createHash("sha256"), modules: string[] = [];
    for (const entry of ENTRIES) {
        const out = await build({
            configFile: false, root: ROOT, logLevel: "silent", ssr: { noExternal: true },
            build: { ssr: entry, write: false, minify: false, rollupOptions: { output: { inlineDynamicImports: true } } },
        });
        const [chunk] = (Array.isArray(out) ? out[0] : out as { output: { code: string; moduleIds: string[] }[] }).output;
        hash.update(chunk.code);
        modules.push(...chunk.moduleIds);
    }
    return { version: hash.digest("hex").slice(0, 16), modules };
}

let bundle: ReturnType<typeof planBundle> | undefined;

/** The dev server reloads the page when a module of the bundle changes. */
export function planVersion(): Plugin {
    const id = "virtual:plan-version", resolved = `\0${id}`;
    return {
        name: "plan-version",
        resolveId: source => (source === id ? resolved : undefined),
        async load(source) {
            if (source !== resolved) return undefined;
            bundle ??= planBundle();
            return `export default ${JSON.stringify((await bundle).version)};`;
        },
        configureServer(server) {
            server.watcher.on("change", async file => {
                const module = server.moduleGraph.getModuleById(resolved);
                if (!module || !bundle || !(await bundle).modules.includes(path.resolve(file))) return;
                bundle = undefined;
                server.moduleGraph.invalidateModule(module);
                server.ws.send({ type: "full-reload" });
            });
        },
    };
}
