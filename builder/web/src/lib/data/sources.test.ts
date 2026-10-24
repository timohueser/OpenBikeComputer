import { expect, it } from "vitest";
import example from "../../../test-support/data/sources.json";
import { coverage, coverageCells, type SourceRow } from "./sources";

const rows = example.sources as SourceRow[];
const row = (id: string) => rows.find((at) => at.id === id)!;
const r2 = (target: string) => target === "prod";

it("a member is where its versions are, unless a run holds or left its lock", () => {
    const states = ["terrain-reference/at", "terrain-reference/ch", "terrain-reference/de", "forest-reference/ch", "wiki"].map((id) => coverage(row(id), r2));
    expect(states).toEqual(["lock", "r2", "preparing", "local", "r2"]);
});

it("the map colours each extent by its versions of all of it, and draws a regional version above", () => {
    const lucerne: [number, number, number, number] = [7.6, 46.46, 8.95, 47.25];
    const cells = coverageCells([row("forest-reference/ch"), row("wiki")], r2, (region) => (region === "central-switzerland" ? lucerne : null));
    expect(cells.map(({ box, state }) => [box, state])).toEqual([
        [[5.9, 45.8, 10.5, 47.9], "none"],
        [lucerne, "local"],
    ]);
});
