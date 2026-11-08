// `virtual:NAME-version`: the version of the JavaScript that a worker runs to make what the device cache keeps,
// the start of the SHA-256 of its entries bundled with every import inlined. `plan`: the plan worker's input
// preparation, with the page decode that it imports, and the input walk; the cells also key by the hash of the
// bridge's wasm, which the bundle only names by URL. `tree`: the tree models. `house`: the house records.

import { createHash } from "node:crypto";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { build, type Plugin } from "vite";

const ROOT = fileURLToPath(new URL("..", import.meta.url));
const ENTRIES: Record<string, string[]> = {
    plan: ["src/lib/trailview/plan/plan-worker.ts", "src/lib/trailview/plan/inputs.ts"],
    tree: ["src/lib/trailview/vegetation/model-worker.ts"],
    house: ["src/lib/trailview/structures/house-worker.ts"],
};

/** The start of the SHA-256 of the entries bundled with every import inlined, and the files of the modules in them. */
async function bundle(entries: string[]): Promise<{ version: string; modules: string[] }> {
    const hash = createHash("sha256"), modules: string[] = [];
    for (const entry of entries) {
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

/** The versions of ENTRIES. The dev server reloads the page when a module of a bundle changes. */
export function bundleVersions(): Plugin {
    const bundles = new Map<string, ReturnType<typeof bundle>>();
    const nameOf = (id: string) => /^\0virtual:(\w+)-version$/.exec(id)?.[1];
    return {
        name: "bundle-versions",
        resolveId: source => (/^virtual:(\w+)-version$/.exec(source)?.[1] ?? "") in ENTRIES ? `\0${source}` : undefined,
        async load(id) {
            const name = nameOf(id);
            if (!name || !(name in ENTRIES)) return undefined;
            if (!bundles.has(name)) bundles.set(name, bundle(ENTRIES[name]));
            return `export default ${JSON.stringify((await bundles.get(name)!).version)};`;
        },
        configureServer(server) {
            server.watcher.on("change", async file => {
                for (const [name, made] of bundles) {
                    const module = server.moduleGraph.getModuleById(`\0virtual:${name}-version`);
                    if (!module || !(await made).modules.includes(path.resolve(file))) continue;
                    bundles.delete(name);
                    server.moduleGraph.invalidateModule(module);
                    server.ws.send({ type: "full-reload" });
                }
            });
        },
    };
}
