/**
 * The USB stack must stay in its own chunk, not the web tier's entry bundle.
 *
 * `platform/web.ts` reaches it through a dynamic `import()` so a visitor who only downloads a map
 * never fetches the transport, the codecs and the client — about 24 kB raw. That split is one
 * ordinary-looking import away from disappearing, and it disappears *silently*: everything still
 * works, the entry chunk is just bigger. So it is asserted against the chunks Rollup actually
 * emitted, the same way A1 asserts the host split in `platform/bundle.test.ts`.
 *
 * **The likeliest way to break it** is deduplication that looks like a tidy-up. C2's
 * `platform/gating.ts` has its own one-line `hasWebUsb()` — `"usb" in navigator` — which overlaps
 * this module's `webUsb()`. They are duplicated **on purpose**: `gating.ts` is imported by the home
 * route and therefore lives in the entry chunk, so importing anything from `lib/usb/` into it would
 * drag the whole stack in behind it. Two probes, two lines, no import edge. If you are here because
 * you were about to merge them, this is the reason not to.
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
        build: { write: false, outDir: "dist/.usb-bundle-test" },
    });
    const result = (Array.isArray(out) ? out[0] : out) as unknown as BuiltChunks;
    return result.output
        .filter((chunk) => chunk.type === "chunk")
        .map((chunk) => ({ isEntry: chunk.isEntry === true, modules: Object.keys(chunk.modules ?? {}) }));
}

const webChunks = () => chunksOf("web");

const IS_USB = /\/src\/lib\/usb\//;

describe("the USB stack's chunk", () => {
    it("is code-split out of the web tier's entry bundle", async () => {
        const chunks = await webChunks();

        // Guard the guard: if the glob stopped matching, every assertion below would pass vacuously.
        const usbModules = chunks.flatMap((c) => c.modules.filter((id) => IS_USB.test(id)));
        expect(usbModules.some((id) => id.endsWith("/src/lib/usb/client.ts"))).toBe(true);
        expect(usbModules.some((id) => id.endsWith("/src/lib/usb/webusb.ts"))).toBe(true);

        const inEntry = chunks
            .filter((c) => c.isEntry)
            .flatMap((c) => c.modules.filter((id) => IS_USB.test(id)));
        expect(inEntry, "the USB stack leaked into the entry chunk").toEqual([]);
    }, 180_000);

    it.each(["web", "desktop"])("does not ship the simulated device (%s target)", async (mode) => {
        // `loopback.ts` is a whole device — an object store, a catalog, id assignment. It exists so
        // the epic isn't blocked on #889's silicon, and it has no business in anything a person
        // installs or visits.
        //
        // The **desktop** row is not symmetry for its own sake. That app is the one people take to
        // a bench with a real board, and D4's (#909) on-glass recipe leans on "if the window says
        // Connected, something enumerated" as its first tell that a transfer is real. A simulated
        // device reachable from the shipped app would make that sentence false — quietly, and
        // exactly when someone is trying to decide whether hardware works.
        const shipped = (await chunksOf(mode)).flatMap((c) =>
            c.modules.filter((id) => /\/src\/lib\/usb\/loopback\.ts$/.test(id)),
        );
        expect(shipped).toEqual([]);
    }, 180_000);
});
