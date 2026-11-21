// `virtual:models`: the model files of TrailView (specs/trailview-models.md) and where each lies. The plugin bundles
// `models/bake.ts` for Node with every import inlined, runs it, and serves what it returns: a build emits each file as an
// asset with the hash of its content in its name; the dev server serves the files itself and makes them again when a
// module of the bundle changes, when a TrailView module is added or removed, or on any change after a bake that failed.

import { createHash } from "node:crypto";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { build, type Plugin } from "vite";
import type { Baked } from "../src/lib/trailview/models/bake";

const ROOT = fileURLToPath(new URL("..", import.meta.url));
const ENTRY = "src/lib/trailview/models/bake.ts";
const ID = "virtual:models";
/** Where the dev server serves the files. */
const PREFIX = "/@trailview-models/";
/** Marks a file name in the manifest's JSON, which the module replaces with the file's address. */
const MARK = /"@@model-file:([\w-]+)@@"/g;
/** The modules that a change makes the files again for, after a bake that failed. */
const SOURCES = path.join(ROOT, "src/lib/trailview");

/** The files and the models, and the files of the modules in the bundle. */
async function bakeModels(): Promise<{ baked: Baked; modules: string[] }> {
    const out = await build({
        configFile: false, root: ROOT, logLevel: "silent", ssr: { noExternal: true },
        build: { ssr: ENTRY, write: false, minify: false, rollupOptions: { output: { inlineDynamicImports: true } } },
    });
    const [chunk] = (Array.isArray(out) ? out[0] : out as { output: { code: string; moduleIds: string[] }[] }).output;
    const dir = await mkdtemp(path.join(tmpdir(), "trailview-models-"));
    try {
        const file = path.join(dir, "bake.mjs");
        await writeFile(file, chunk.code);
        const { bake } = await import(pathToFileURL(file).href) as { bake: () => Promise<Baked> };
        return { baked: await bake(), modules: chunk.moduleIds };
    } finally {
        await rm(dir, { recursive: true, force: true });
    }
}

export function modelFiles(): Plugin {
    let made: ReturnType<typeof bakeModels> | undefined, serving = false;
    // A bake that fails leaves no files: the next request or change bakes again.
    const bake = () => (made ??= bakeModels().catch(error => {
        made = undefined;
        throw error;
    }));
    const resolved = `\0${ID}`;
    return {
        name: "trailview-models",
        configResolved: config => void (serving = config.command === "serve"),
        resolveId: source => (source === ID ? resolved : undefined),
        async load(id) {
            if (id !== resolved) return undefined;
            const { baked: { files, models } } = await bake();
            const json = JSON.stringify(models, (_, value) => (typeof value === "string" && files.has(value) ? `@@model-file:${value}@@` : value));
            return `export default ${json.replace(MARK, (_, name: string) => {
                const bytes = files.get(name)!;
                if (serving) return JSON.stringify(`${PREFIX}${name}?v=${createHash("sha256").update(bytes).digest("hex").slice(0, 12)}`);
                return `import.meta.ROLLUP_FILE_URL_${this.emitFile({ type: "asset", name: `${name}.bin`, source: bytes })}`;
            })};`;
        },
        configureServer(server) {
            server.middlewares.use(PREFIX, (request, response, next) => {
                void bake().then(({ baked: { files } }) => {
                    const bytes = files.get((request.url ?? "").slice(1).split("?")[0]);
                    if (!bytes) return next();
                    response.setHeader("Content-Type", "application/octet-stream");
                    response.end(bytes);
                }, next);
            });
            // A changed module of the bake, or any added or removed TrailView module (a new tree or plant file of a glob)
            // makes the files again.
            const again = async (file: string, added: boolean) => {
                const module = server.moduleGraph.getModuleById(resolved);
                if (!module) return;
                const modules = added ? undefined : await made?.then(({ modules }) => modules, () => undefined);
                if (modules ? !modules.includes(path.resolve(file)) : !path.resolve(file).startsWith(SOURCES)) return;
                made = undefined;
                server.moduleGraph.invalidateModule(module);
                server.ws.send({ type: "full-reload" });
            };
            server.watcher.on("change", file => void again(file, false));
            for (const event of ["add", "unlink"] as const) server.watcher.on(event, file => void again(file, true));
        },
    };
}
