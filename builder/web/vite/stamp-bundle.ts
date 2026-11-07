// The stamp bake as one ES module for Node, and its version: the bake of `planner/trailview-stamps` runs the
// module, and the viewer reads a baked node only when the layer's `stamp_version` is the version of its build.
//
// This module imports nothing local: scripts/trailview-stamps.mjs loads it in plain Node.

import { createHash } from "node:crypto";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { build, type Plugin } from "vite";

const ROOT = fileURLToPath(new URL("..", import.meta.url));
/** The bake entry: the stamp worker, the node bytes and the baked nodes of a feature tile. */
const ENTRY = "src/lib/trailview/terrain/stamp-bake.ts";

/**
 * The module of the bake with every import inlined, its version (the start of the SHA-256 of the module, so of
 * the stamp code and the tables that it reads) and the files of the modules in it.
 */
export async function stampBundle(): Promise<{ code: string; version: string; modules: string[] }> {
    const out = await build({
        configFile: false, root: ROOT, logLevel: "silent", ssr: { noExternal: true },
        build: { ssr: ENTRY, write: false, minify: false, rollupOptions: { output: { inlineDynamicImports: true } } },
    });
    const [chunk] = (Array.isArray(out) ? out[0] : out as { output: { code: string; moduleIds: string[] }[] }).output;
    return { code: chunk.code, version: createHash("sha256").update(chunk.code).digest("hex").slice(0, 16), modules: chunk.moduleIds };
}

/** `virtual:stamp-version`: the version of stampBundle. The dev server makes it again and reloads the page when a module of the bake changes. */
export function stampVersion(): Plugin {
    const id = "virtual:stamp-version", resolved = `\0${id}`;
    let modules = new Set<string>();
    return {
        name: "stamp-version",
        resolveId: source => (source === id ? resolved : undefined),
        async load(source) {
            if (source !== resolved) return undefined;
            const bundle = await stampBundle();
            modules = new Set(bundle.modules);
            return `export default ${JSON.stringify(bundle.version)};`;
        },
        configureServer(server) {
            server.watcher.on("change", file => {
                const module = server.moduleGraph.getModuleById(resolved);
                if (!module || !modules.has(path.resolve(file))) return;
                server.moduleGraph.invalidateModule(module);
                server.ws.send({ type: "full-reload" });
            });
        },
    };
}
