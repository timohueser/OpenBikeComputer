/**
 * The preset-preview renderer must stay out of every entry chunk.
 *
 * This is the acceptance criterion from #899 ("preview assets are small enough not to dominate
 * first paint") turned into something a build can fail on. The module is ~60 kB gzipped —
 * comparable to the whole rest of the app — and it exists to draw three pictures a visitor may
 * never scroll to. `PresetPreview.svelte` therefore reaches it through dynamic `import()`s inside
 * an IntersectionObserver callback, which is exactly the kind of split that disappears *silently*:
 * turn one of those into a top-level import and everything still works, the entry chunk is just
 * bigger and slower. So it is asserted against the chunks Rollup actually emitted, the same way
 * A1 asserts the host split and C3 the USB stack's.
 *
 * **The likeliest way to break it** is reaching for a type. `import type { Preview }` is erased
 * and costs nothing; dropping the `type` keyword, or importing a *value* (a constant, an error
 * class) from `lib/preview/bridge.ts` or `demoMaps.ts` into the component, pulls the whole
 * renderer into the page's first paint. `lib/preview/copy.ts` is deliberately importable — it is
 * three strings with no dependencies, and the card needs them before any picture exists.
 */

import { build } from "vite";
import { describe, expect, it } from "vitest";

/** The single-input slice of Rollup's result we need — vite re-exports the builder, not this type. */
interface BuiltChunks {
    output: Array<{ type: string; isEntry?: boolean; modules?: Record<string, unknown> }>;
}

/** Every emitted chunk of one target, as `{ isEntry, modules }`. */
async function chunksOf(mode: string): Promise<Array<{ isEntry: boolean; modules: string[] }>> {
    const out = await build({
        mode,
        logLevel: "error",
        build: { write: false, outDir: "dist/.preview-bundle-test" },
    });
    const result = (Array.isArray(out) ? out[0] : out) as unknown as BuiltChunks;
    return result.output
        .filter((chunk) => chunk.type === "chunk")
        .map((chunk) => ({ isEntry: chunk.isEntry === true, modules: Object.keys(chunk.modules ?? {}) }));
}

/** The renderer and its map loader — everything a card must not pay for up front. */
const IS_LAZY = /\/src\/lib\/preview\/(bridge|demoMaps)\.ts$|\/src\/lib\/preview\/pkg\//;
/** The copy, which the card reads on first paint and which pulls in nothing. */
const IS_COPY = /\/src\/lib\/preview\/copy\.ts$/;

describe("the preset-preview renderer's chunk", () => {
    it.each(["web", "production", "desktop"])("is code-split out of the entry bundle (%s)", async (mode) => {
        const chunks = await chunksOf(mode);

        // Guard the guard: if the glob stopped matching, the assertion below would pass vacuously
        // on a build that simply no longer contains the renderer at all.
        const lazy = chunks.flatMap((c) => c.modules.filter((id) => IS_LAZY.test(id)));
        expect(lazy.some((id) => id.endsWith("/src/lib/preview/bridge.ts"))).toBe(true);
        expect(lazy.some((id) => id.endsWith("/src/lib/preview/demoMaps.ts"))).toBe(true);

        const inEntry = chunks
            .filter((c) => c.isEntry)
            .flatMap((c) => c.modules.filter((id) => IS_LAZY.test(id)));
        expect(inEntry, "the preview renderer leaked into the entry chunk").toEqual([]);
    }, 180_000);

    it("still ships the preset copy up front", async () => {
        // The other half of the claim: the split is about the renderer, not about the words. A
        // card must be able to say what a preset is for before a single byte of wasm arrives.
        const chunks = await chunksOf("web");
        const inEntry = chunks.filter((c) => c.isEntry).flatMap((c) => c.modules.filter((id) => IS_COPY.test(id)));
        expect(inEntry.length, "the preset copy is not in the entry chunk").toBeGreaterThan(0);
    }, 180_000);
});
