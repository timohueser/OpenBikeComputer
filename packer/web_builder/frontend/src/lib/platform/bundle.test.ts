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

/**
 * Source modules the static web tier must not contain, and why.
 *
 * The Tauri rows are not the same claim as the rest: `@tauri-apps/api` in a
 * static site's bundle would be dead weight *and* a lie about where the app
 * runs, but more usefully, its presence would mean the host alias picked the
 * wrong module — which the assertions below could not otherwise notice, because
 * a desktop host's methods all resolve and simply never work in a browser.
 * (`lib/desktop/release.ts` is deliberately absent from this list: the *web*
 * tier reads it, to decide whether the desktop app has a download link yet.)
 */
const DESKTOP_ONLY = {
    "the FastAPI client": /\/src\/lib\/api\/client\.ts$/,
    "the SSE job tracker": /\/src\/lib\/api\/jobs\.svelte\.ts$/,
    "the dev host": /\/src\/lib\/platform\/dev\.ts$/,
    "the desktop host": /\/src\/lib\/platform\/desktop\.ts$/,
    "the Tauri command bridge": /\/src\/lib\/desktop\/invoke\.ts$/,
    "the Tauri build tracker": /\/src\/lib\/desktop\/build\.svelte\.ts$/,
    "the Tauri JS API": /\/@tauri-apps\/api\//,
    "the style editor route": /\/src\/routes\/Advanced\.svelte/,
    "the style editor components": /\/src\/components\/advanced\//,
};

/** …and the ones only the *desktop* target may contain, for the same reason in
 *  the other direction: a desktop build that quietly fell back to the dev host
 *  would talk to a FastAPI server that isn't running. */
const DESKTOP_TARGET_ONLY = {
    "the desktop host": /\/src\/lib\/platform\/desktop\.ts$/,
    "the Tauri command bridge": /\/src\/lib\/desktop\/invoke\.ts$/,
    "the Tauri build tracker": /\/src\/lib\/desktop\/build\.svelte\.ts$/,
    "the Tauri JS API": /\/@tauri-apps\/api\//,
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

    it("still ships the build and style-editor code in the dev target", async () => {
        // The dev host's own set: everything in DESKTOP_ONLY except the three
        // rows that name the *desktop* host, which the dev target must not have
        // either — one alias picks exactly one host.
        const modules = await bundledModules("production");
        const missing = Object.entries(DESKTOP_ONLY)
            .filter(([what]) => !(what in DESKTOP_TARGET_ONLY))
            .filter(([, re]) => !modules.some((id) => re.test(id)))
            .map(([what]) => what);
        expect(missing).toEqual([]);

        const strays = Object.entries(DESKTOP_TARGET_ONLY).flatMap(([what, re]) =>
            modules.filter((id) => re.test(id)).map((id) => `${what}: ${id}`),
        );
        expect(strays).toEqual([]);
    }, 180_000);

    it("wires the desktop target to the Tauri backend", async () => {
        const modules = await bundledModules("desktop");
        const missing = Object.entries(DESKTOP_TARGET_ONLY)
            .filter(([, re]) => !modules.some((id) => re.test(id)))
            .map(([what]) => what);
        expect(missing).toEqual([]);
        // …and not to the dev server's, which is the failure this catches: both
        // hosts can build, so a wrong alias produces an app that looks right and
        // POSTs to a localhost that isn't there.
        expect(modules.some((id) => /\/src\/lib\/api\/client\.ts$/.test(id))).toBe(false);
    }, 180_000);
});
