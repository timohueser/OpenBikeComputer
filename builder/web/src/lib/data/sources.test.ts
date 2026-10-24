import { expect, it } from "vitest";
import example from "../../../test-support/data/sources.json";
import { coverage, coverageCells, modelsOf, type SourceRow } from "./sources";

const rows = example.sources as SourceRow[];
const row = (id: string) => rows.find((at) => at.id === id)!;
const r2 = (target: string) => target === "prod";

it("a reference is where its versions are, unless a run holds or left its lock", () => {
    const states = ["forest", "terrain", "wiki"].map((id) => coverage(row(id), r2));
    expect(states).toEqual(["lock", "r2", "r2"]);
});

it("the map draws the extent of each national model, and the area of each version above it", () => {
    const lucerne: [number, number, number, number] = [7.6, 46.46, 8.95, 47.25];
    const models = modelsOf("forest", rows);
    expect(models.map(({ row, country }) => [row.id, country])).toEqual([["canopy-ch", "CH"]]);
    const cells = coverageCells(row("forest"), models.map((at) => at.row), r2, (region) => (region === "central-switzerland" ? lucerne : null));
    expect(cells.map(({ box, state }) => [box, state])).toEqual([
        [[5.9, 45.8, 10.5, 47.9], "none"],
        [lucerne, "local"],
    ]);
});
