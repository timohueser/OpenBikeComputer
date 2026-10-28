import { expect, it } from "vitest";
import example from "../../../test-support/data/sources.json";
import { coverage, coverageCells, modelsOf, type SourceRow } from "./sources";

const rows = example.sources as SourceRow[];
const row = (id: string) => rows.find((at) => at.id === id)!;

it("a reference is in the archive when it has versions, unless a run holds or left its lock", () => {
    const states = ["forest", "terrain", "wiki", "osm-planet"].map((id) => coverage(row(id)));
    expect(states).toEqual(["lock", "archive", "archive", "none"]);
});

it("the map draws the extent of each national model, and the area of each version above it", () => {
    const lucerne: [number, number, number, number] = [7.6, 46.46, 8.95, 47.25];
    const models = modelsOf("forest", rows);
    expect(models.map(({ row, country }) => [row.id, country])).toEqual([["canopy-ch", "CH"]]);
    const cells = coverageCells(row("forest"), models.map((at) => at.row), (region) => (region === "central-switzerland" ? lucerne : null));
    expect(cells.map(({ box, state }) => [box, state])).toEqual([
        [[5.9, 45.8, 10.5, 47.9], "none"],
        [lucerne, "archive"],
    ]);
});
