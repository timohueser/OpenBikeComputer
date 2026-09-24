// @vitest-environment happy-dom

import { mount, tick, unmount } from "svelte";
import { afterEach, describe, expect, it } from "vitest";
import type { CoverageStore } from "../../lib/coverage/store.svelte";
import MapSummary from "./MapSummary.svelte";

afterEach(() => document.body.replaceChildren());

describe("MapSummary", () => {
    it("shows all coverage gaps and both data credits before a build", async () => {
        const store = {
            selection: { parts: [{ id: "area" }] },
            ledger: {
                isFinal: true,
                totalBytes: 4_096,
                cellCount: 3,
                terrain: {
                    bytes: 1_024,
                    missingCount: 2,
                    attribution: "Terrain source credit",
                    references: [],
                },
            },
            catalog: {
                source: {
                    attribution: "Map source credit",
                    license: "Map licence",
                    license_url: "https://example.org/licence",
                },
            },
            holeCells: () => ["hole-1", "hole-2"],
            partialDetailCells: () => ["partial-1"],
            partialHatchCells: () => ["partial-1"],
        } as unknown as CoverageStore;
        const target = document.createElement("div");
        document.body.append(target);
        const component = mount(MapSummary, { target, props: { store } });
        await tick();
        const content = (element: Element) => element.textContent?.replace(/\s+/g, " ").trim();

        expect(content(target.querySelector(".total")!)).toContain("4.0 KB · 3 cells");
        expect([...target.querySelectorAll(".warnline")].map(content)).toEqual([
            expect.stringContaining("2 cells not baked yet"),
            expect.stringContaining("1 cell is only partly baked"),
        ]);
        expect(content(target.querySelector(".terrain")!)).toContain("2 squares have no elevation coverage");
        expect([...target.querySelectorAll(".attribution")].map(content)).toEqual([
            "Terrain source credit",
            expect.stringContaining("Map source credit"),
        ]);
        expect(content(target.querySelector(".fit")!)).toContain("one map file");
        await unmount(component);
    });
});
