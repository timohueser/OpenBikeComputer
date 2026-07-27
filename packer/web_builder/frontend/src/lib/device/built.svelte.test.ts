/**
 * The handover between the build card and the device step (E3, #913).
 *
 * Small, and worth pinning anyway: both rules here exist because getting them wrong puts the
 * *wrong file* on someone's card. The build card announces from an effect that re-runs on every
 * unrelated change, and it clears the holder the moment a new build starts.
 */

import { beforeEach, describe, expect, it } from "vitest";

import { builtMap } from "./built.svelte";

const MAP = { path: "/Users/x/OpenBikeComputer/black-forest.obcm", filename: "black-forest.obcm", bytes: 41_000_000 };

beforeEach(() => builtMap.clear());

describe("the map this app just built", () => {
    it("is nothing until a build has produced one", () => {
        expect(builtMap.current).toBeNull();
    });

    it("re-announcing the same file does not replace it", async () => {
        builtMap.note(MAP);
        const first = builtMap.current;
        await new Promise((resolve) => setTimeout(resolve, 2));
        builtMap.note(MAP);
        // Identity, not just equality: a fresh object would restart anything keyed on it — a "sent"
        // badge, a transfer the rider is watching — for a build that did not happen.
        expect(builtMap.current).toBe(first);
    });

    it("a different build replaces it", () => {
        builtMap.note(MAP);
        builtMap.note({ ...MAP, path: `${MAP.path}-2`, filename: "black-forest-2.obcm" });
        expect(builtMap.current?.filename).toBe("black-forest-2.obcm");
    });

    it("is cleared when a build starts, so no button points at the previous map", () => {
        builtMap.note(MAP);
        builtMap.clear();
        expect(builtMap.current).toBeNull();
    });
});
