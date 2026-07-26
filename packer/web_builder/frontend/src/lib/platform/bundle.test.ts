// The bundle-split guard for #895: the static web tier must not ship the
// FastAPI job-polling client or the desktop-only style editor.
//
// This asserts against the modules Rollup actually put in the emitted chunks,
// not against source imports or a grep of the output — the claim is about the
// build product, so the build product is what gets inspected. Both targets are
// built: if the web assertions ever pass because the glob stopped matching
// anything, the dev assertions fail in the same run.

import { build } from "vite";
import { describe, expect, it } from "vitest";

/** The single-input slice of Rollup's result we need — vite re-exports the
 *  builder but not this type. */
interface BuiltChunks {
    output: Array<{ type: string; modules?: Record<string, unknown> }>;
}

/** Source modules the static web tier must not contain, and why. */
const DESKTOP_ONLY = {
    "the FastAPI client": /\/src\/lib\/api\/client\.ts$/,
    "the SSE job tracker": /\/src\/lib\/api\/jobs\.svelte\.ts$/,
    "the dev host": /\/src\/lib\/platform\/dev\.ts$/,
    "the style editor route": /\/src\/routes\/Advanced\.svelte/,
    "the style editor components": /\/src\/components\/advanced\//,
};

/**
 * Every source module in every emitted chunk, for one build target. `write:
 * false` keeps this off disk; outDir is redirected anyway so a future Vite that
 * prepares the directory before writing can't wipe a real build.
 */
async function bundledModules(mode: string): Promise<string[]> {
    const out = await build({
        mode,
        logLevel: "error",
        build: { write: false, outDir: "dist/.bundle-test" },
    });
    const result = (Array.isArray(out) ? out[0] : out) as unknown as BuiltChunks;
    const ids: string[] = [];
    for (const chunk of result.output) {
        if (chunk.type === "chunk") ids.push(...Object.keys(chunk.modules ?? {}));
    }
    return ids;
}

describe("bundle split", () => {
    it("keeps build and style-editor code out of the web target", async () => {
        const modules = await bundledModules("web");
        // An empty build, or one that quietly picked another host, would pass
        // every assertion below.
        expect(modules.some((id) => id.endsWith("/src/lib/platform/web.ts"))).toBe(true);
        expect(modules.some((id) => id.includes("/src/routes/Home.svelte"))).toBe(true);

        const found = Object.entries(DESKTOP_ONLY).flatMap(([what, re]) =>
            modules.filter((id) => re.test(id)).map((id) => `${what}: ${id}`),
        );
        expect(found).toEqual([]);
    }, 180_000);

    it("still ships them in the dev target", async () => {
        const modules = await bundledModules("production");
        const missing = Object.entries(DESKTOP_ONLY)
            .filter(([, re]) => !modules.some((id) => re.test(id)))
            .map(([what]) => what);
        expect(missing).toEqual([]);
    }, 180_000);
});
