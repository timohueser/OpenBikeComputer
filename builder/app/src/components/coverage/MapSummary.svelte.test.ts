// @vitest-environment happy-dom

import { mount, tick, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
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
            focusWarnings: vi.fn(),
        } as unknown as CoverageStore;
        const target = document.createElement("div");
        document.body.append(target);
        const component = mount(MapSummary, { target, props: { store } });
        await tick();
        const content = (element: Element) => element.textContent?.replace(/\s+/g, " ").trim();

        expect(content(target.querySelector(".total")!)).toContain("4.0 KB estimated total");
        expect([...target.querySelectorAll(".warnline")].map(content)).toEqual([
            expect.stringContaining("Map data is missing in some selected areas"),
            expect.stringContaining("Street detail may stop near these gaps"),
        ]);
        expect(content(target.querySelector(".terrain")!)).toContain("Elevation data is missing in some selected areas");
        expect([...target.querySelectorAll(".attribution")].map(content)).toEqual([
            "Terrain source credit",
            expect.stringContaining("Map source credit"),
        ]);
        expect(content(target.querySelector(".fit")!)).toContain("one map file");
        const warnings = target.querySelectorAll<HTMLButtonElement>("button.warnline");
        warnings[0].click();
        warnings[1].click();
        expect(store.focusWarnings).toHaveBeenNthCalledWith(1, "hole");
        expect(store.focusWarnings).toHaveBeenNthCalledWith(2, "partial");
        expect(target.textContent).not.toContain("1.0 KB");
        await unmount(component);
    });

    it.each([
        { label: "a region border", kind: "region", drawnCells: ["border"], warns: false },
        ...["box", "lasso", "corridor"].flatMap((kind) => [
            { label: `a region and a fully covered ${kind}`, kind, drawnCells: ["full"], warns: false },
            { label: `a region and a partly covered ${kind}`, kind, drawnCells: ["full", "border"], warns: true },
        ]),
    ])("discloses only affected drawn coverage for $label", async ({ kind, drawnCells, warns }) => {
        const region = { id: "region", kind: "region" };
        const drawn = { id: "drawn", kind };
        const store = {
            selection: { parts: [region, drawn] },
            resolution: {
                parts: [
                    { part: region, cellsByBand: new Map([["fine", ["full", "border"]]]) },
                    { part: drawn, cellsByBand: new Map([["fine", drawnCells]]) },
                ],
            },
            ledger: { isFinal: true, totalBytes: 4_096, cellCount: 3, terrain: null },
            catalog: {
                schema: { bands: [{ id: "fine", role: "fine", lods: [0], cell_log2: 18 }] },
            },
            holeCells: () => [],
            partialDetailCells: () => ["border"],
            partialHatchCells: () => [],
        } as unknown as CoverageStore;
        const target = document.createElement("div");
        document.body.append(target);
        const component = mount(MapSummary, { target, props: { store } });
        await tick();
        expect(target.querySelectorAll(".warnline")).toHaveLength(warns ? 1 : 0);
        if (warns) expect(target.textContent).toContain("Street detail may be incomplete");
        expect(target.querySelector(".total")?.textContent).toContain("4.0 KB");
        await unmount(component);
    });
});
