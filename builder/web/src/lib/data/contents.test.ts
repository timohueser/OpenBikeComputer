import { expect, it } from "vitest";
import { CONTENTS, layerFlags, needs, toggled, unshown } from "./contents";
import type { SourceRow } from "./sources";

const card = (id: string) => CONTENTS.find((content) => content.id === id)!;
const versions = (target: string | null) =>
    Object.fromEntries(["local", "prod"].map((name) => [name, { versions: name === target ? [{ version: "2026-10-24", bytes: 1, time: "" }] : [], lock: null, error: null }]));
const source = (id: string, extra: Partial<SourceRow> = {}) => ({ id, credential_missing: false, prepared: {}, ...extra }) as SourceRow;
const lucerne = { west: 8.2, south: 46.9, east: 8.4, north: 47.1 };
const swiss = [5.9, 45.8, 10.5, 47.9] as [number, number, number, number];

it("a card that needs a reference cannot bake until a version covers the area", () => {
    const forest = (target: string | null) => [source("forest-reference/ch", { extent: swiss, use: "Swiss forest", prepared: versions(target) })];
    expect(needs(card("trailview-canopy"), lucerne, forest(null))).toMatchObject({ ready: false, references: [{ source: "forest-reference/ch", version: null }] });
    expect(needs(card("trailview-canopy"), lucerne, forest("local")).ready).toBe(true);
    expect(needs(card("trailview-canopy"), { west: 11, south: 48, east: 12, north: 49 }, forest("prod"))).toMatchObject({ ready: false, references: [] });
});

it("terrain falls back without a reference, a prod version wins, and a missing login blocks", () => {
    const terrain = [source("terrain-reference/ch", { extent: swiss, prepared: { ...versions("prod"), local: versions("local").local } })];
    expect(needs(card("device"), lucerne, terrain)).toMatchObject({ ready: true, references: [{ version: { target: "prod" } }] });
    expect(needs(card("device"), lucerne, []).ready).toBe(true);
    expect(needs(card("climate"), lucerne, [source("era5-land", { credential_missing: true })])).toMatchObject({ ready: false, credentials: ["era5-land"] });
});

it("the flags switch exactly the layers of the cards that are on, and off the others", () => {
    const flags = layerFlags(["trailview", "sun"]);
    expect(flags.filter(([flag]) => flag === "--add").map(([, layer]) => layer)).toEqual([
        "trailview-features",
        "trailview-rules",
        "trailview-ground",
        "trailview-colour",
        "sun",
    ]);
    expect(flags.filter(([flag]) => flag === "--remove").map(([, layer]) => layer)).toEqual([
        "device",
        "map",
        "routing",
        "search",
        "trailview-canopy",
        "snow",
        "climate",
    ]);
});

it("a card needs a card that shows it, and goes when the last one goes", () => {
    expect(unshown(card("sun"), ["routing"])).toBe("Shows in Planner map: switch it on first.");
    expect(unshown(card("snow"), ["trailview"])).toBeNull();
    expect(toggled(["map", "trailview", "sun", "snow", "trailview-canopy"], "map", false)).toEqual(["trailview", "snow", "trailview-canopy"]);
    expect(toggled(["trailview", "snow", "trailview-canopy"], "trailview", false)).toEqual([]);
});

it("an area that names its countries meets only their references", () => {
    const bavaria = [source("terrain-reference/de-by", { extent: [8.9, 47.2, 13.9, 50.6], prepared: versions(null) })];
    expect(needs(card("device"), lucerne, bavaria, ["CH"]).references).toEqual([]);
    expect(needs(card("device"), { west: 8.5, south: 47.1, east: 9.2, north: 47.5 }, bavaria).references).toHaveLength(1);
});
