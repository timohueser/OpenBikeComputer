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
    output: Array<{ type: string; isEntry?: boolean; modules?: Record<string, unknown> }>;
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
    "the dev host": /\/src\/lib\/platform\/dev\.ts$/,
    "the desktop host": /\/src\/lib\/platform\/desktop\.ts$/,
    "the Tauri command bridge": /\/src\/lib\/desktop\/invoke\.ts$/,
    "the native USB transport": /\/src\/lib\/desktop\/usb\.ts$/,
    "the native USB session": /\/src\/lib\/desktop\/usb\.svelte\.ts$/,
    "the ride library's Tauri backing": /\/src\/lib\/desktop\/library\.ts$/,
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
    // D4 (#909): the desktop tier drives USB natively, so the Rust-backed byte
    // pipe must be in this bundle and nowhere else. Its presence here is also
    // what proves the dynamic `import()` in `desktop.ts` still resolves —
    // `device()` is only ever called from a click, so a broken path would
    // otherwise surface on someone's desk.
    "the native USB transport": /\/src\/lib\/desktop\/usb\.ts$/,
    "the native USB session": /\/src\/lib\/desktop\/usb\.svelte\.ts$/,
    // E2 (#912): the ride library is a folder on a disk, so only the tier with
    // one may carry the code that writes it. Its presence here is also what
    // proves `desktop.ts`'s dynamic `import()` still resolves — `rides()` is
    // only called from a click, so a broken path would surface on someone's
    // desk rather than in CI.
    "the ride library's Tauri backing": /\/src\/lib\/desktop\/library\.ts$/,
    "the Tauri JS API": /\/@tauri-apps\/api\//,
};

/**
 * Every source module in every emitted chunk, for one build target. `write:
 * false` keeps this off disk; outDir is redirected anyway so a future Vite that
 * prepares the directory before writing can't wipe a real build.
 */
async function bundledModules(mode: string): Promise<{ all: string[]; entry: string[] }> {
    const out = await build({
        mode,
        logLevel: "error",
        build: { write: false, outDir: "dist/.bundle-test" },
    });
    const result = (Array.isArray(out) ? out[0] : out) as unknown as BuiltChunks;
    const all: string[] = [];
    const entry: string[] = [];
    for (const chunk of result.output) {
        if (chunk.type !== "chunk") continue;
        all.push(...Object.keys(chunk.modules ?? {}));
        if (chunk.isEntry) entry.push(...Object.keys(chunk.modules ?? {}));
    }
    return { all, entry };
}

describe("bundle split", () => {
    it("keeps build and style-editor code out of the web target", async () => {
        const { all: modules } = await bundledModules("web");
        // An empty build, or one that quietly picked another host, would pass
        // every assertion below.
        expect(modules.some((id) => id.endsWith("/src/lib/platform/web.ts"))).toBe(true);
        expect(modules.some((id) => id.includes("/src/routes/Home.svelte"))).toBe(true);

        const found = Object.entries(DESKTOP_ONLY).flatMap(([what, re]) =>
            modules.filter((id) => re.test(id)).map((id) => `${what}: ${id}`),
        );
        expect(found).toEqual([]);

        // The cell composer is now the hosted builder rather than a lazily
        // loaded alternative selected by a manifest probe.
        expect(modules.some((id) => id.includes("/src/components/coverage/CoverageHome.svelte"))).toBe(
            true,
        );
    }, 180_000);

    it("ships the maintainer style editor only in the dev target", async () => {
        // The dev host's own set: everything in DESKTOP_ONLY except the three
        // rows that name the *desktop* host, which the dev target must not have
        // either — one alias picks exactly one host.
        const { all: modules } = await bundledModules("production");
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
        const { all: modules } = await bundledModules("desktop");
        const missing = Object.entries(DESKTOP_TARGET_ONLY)
            .filter(([, re]) => !modules.some((id) => re.test(id)))
            .map(([what]) => what);
        expect(missing).toEqual([]);
        // …and not to the maintainer dev server.
        expect(modules.some((id) => /\/src\/lib\/api\/client\.ts$/.test(id))).toBe(false);
    }, 180_000);
});
