import { expect, it } from "vitest";
import example from "../../../test-support/data/sources-region.json";
import sources from "../../../test-support/data/sources.json";
import { fill, versionCells, type Held, type RegionSources } from "./holding";
import type { SourceRow } from "./sources";

const rows = sources.sources as SourceRow[];
const row = (id: string, held: Partial<Held> = {}): SourceRow => {
    const at = rows.find((at) => at.id === id)!;
    return { ...at, held: { ...at.held, ...held } };
};

it("a gap names what fills it: the prepare that reads a national model, or a dataset for data/archive/", () => {
    expect(fill(row("dtm-ch", { status: "missing", prepared_by: null, gap: true }))).toEqual({
        text: "The prepare of the source that reads it fetches it. Until then the terrain there is GLO-30 only.",
    });
    expect(fill(row("natural-earth", { status: "missing", dataset: null, no_dataset: true }))).toEqual({ text: "No dataset of data/archive/ lists it. Add one, then get it." });
    expect(fill(row("copernicus-glo-30"))?.command).toEqual(["archive", "get", "copernicus-glo-30"]);
    expect(fill(row("dtm-ch")), "a prepared input has no gap").toBeNull();
});

it("a version covers the region only when the source is held there and its box holds the region", () => {
    const lucerne = { west: 7.6, south: 46.46, east: 8.95, north: 47.25 };
    const need = (source: string, at: string) => ({ ...example.sources[0], source, for_region: { ...example.sources[0].for_region, need: at } });
    const region = (boxes: RegionSources["boxes"], ...needs: [string, string][]) =>
        ({ ...example, boxes, sources: needs.map(([source, at]) => need(source, at)) }) as RegionSources;
    const kinds = (at: RegionSources) => versionCells(at, rows, (id) => (id === "central-switzerland" ? lucerne : null)).map((cell) => [cell.label, cell.kind]);
    expect(kinds(region(example.boxes, ["terrain", "held"], ["forest", "prepare"]))).toEqual([
        ["terrain 2026-08-20 · central-switzerland", "covers"],
        ["forest 2026-10-20 · central-switzerland", "other"],
    ]);
    const geneva = [{ west: 6.0, south: 46.1, east: 6.3, north: 46.3 }];
    expect(kinds(region(geneva, ["terrain", "held"], ["forest", "unknown"]))).toEqual([
        ["terrain 2026-08-20 · central-switzerland", "other"],
        ["forest 2026-10-20 · central-switzerland", "unknown"],
    ]);
});
