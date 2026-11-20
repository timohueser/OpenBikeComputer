// The stamp bake as one ES module for Node, and its version: the bake of `planner/trailview-stamps` runs the
// module, and the viewer reads a baked node only when the layer's `stamp_version` is the version of its build.
//
// This module imports nothing local: scripts/trailview-stamps.mjs loads it in plain Node.

import { createHash } from "node:crypto";
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { build, type Plugin } from "vite";

const ROOT = fileURLToPath(new URL("..", import.meta.url));
const REPO = path.join(ROOT, "../..");
/** The bake entry: the stamp worker, the node bytes and the baked nodes of a feature tile. */
const ENTRY = "src/lib/trailview/terrain/stamp-bake.ts";
/**
 * The parts bridge, which the stamp code inlines: the hash of the sources that built its package, and those sources.
 * builder/build-wasm-bridges.sh writes the hash when it builds the package.
 */
const PARTS_BUILT = "src/lib/trailview/structures/pkg/sources.sha256";
const PARTS_SOURCES = ["host/obc-trailview-parts", "builder/trailview-parts-wasm", "data/trailview/parts.json"];
const WASM = "data:application/wasm;base64,";

const sha256 = (data: string | Buffer) => createHash("sha256").update(data).digest("hex");

/** The hash of the parts bridge's sources: the SHA-256 of the lines `<SHA-256>  <path>` of their files, by path from the repository. */
export function partsSources(): string {
    const files: string[] = [];
    const walk = (file: string): void => {
        if (statSync(path.join(REPO, file)).isDirectory()) for (const name of readdirSync(path.join(REPO, file))) walk(`${file}/${name}`);
        else files.push(file);
    };
    PARTS_SOURCES.forEach(walk);
    return sha256(files.sort().map(file => `${sha256(readFileSync(path.join(REPO, file)))}  ${file}\n`).join(""));
}

/**
 * The version of the bake's code: the start of its SHA-256, with each inlined WASM module replaced by the hash of
 * the parts bridge's sources, so two builds of the same sources give one version. It refuses a package whose
 * recorded sources (`recorded`) are not `sources`.
 */
export function versionOf(code: string, recorded: string | undefined, sources: string): string {
    if (!code.includes(WASM)) return sha256(code).slice(0, 16);
    if (recorded?.trim() !== sources) throw new Error(`stampBundle: ${PARTS_BUILT} ${recorded === undefined ? "is missing" : "names other sources"}; run bash builder/build-wasm-bridges.sh`);
    return sha256(code.replace(/data:application\/wasm;base64,[A-Za-z0-9+/=]*/g, sources)).slice(0, 16);
}

/**
 * The module of the bake with every import inlined, its version (`versionOf`: of the stamp code, the tables that it
 * reads and the sources of the WASM that it inlines) and the files of the modules in it.
 */
export async function stampBundle(): Promise<{ code: string; version: string; modules: string[] }> {
    const out = await build({
        configFile: false, root: ROOT, logLevel: "silent", ssr: { noExternal: true },
        build: { ssr: ENTRY, write: false, minify: false, rollupOptions: { output: { inlineDynamicImports: true } } },
    });
    const [chunk] = (Array.isArray(out) ? out[0] : out as { output: { code: string; moduleIds: string[] }[] }).output;
    const built = path.join(ROOT, PARTS_BUILT), inlines = chunk.code.includes(WASM);
    const version = versionOf(chunk.code, existsSync(built) ? readFileSync(built, "utf8") : undefined, inlines ? partsSources() : "");
    return { code: chunk.code, version, modules: chunk.moduleIds };
}

/** The bundle of the build: the page and each worker read one version. The dev server drops it when a module of the bake changes. */
let bundle: ReturnType<typeof stampBundle> | undefined;

/** `virtual:stamp-version`: the version of stampBundle. The dev server reloads the page when a module of the bake changes. */
export function stampVersion(): Plugin {
    const id = "virtual:stamp-version", resolved = `\0${id}`;
    return {
        name: "stamp-version",
        resolveId: source => (source === id ? resolved : undefined),
        async load(source) {
            if (source !== resolved) return undefined;
            bundle ??= stampBundle();
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
