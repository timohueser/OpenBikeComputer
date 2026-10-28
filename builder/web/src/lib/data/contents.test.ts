import { expect, it } from "vitest";
import { CONTENTS, layerFlags, needs, TERRAIN, toggled, unshown } from "./contents";
import type { SourceRow } from "./sources";

const card = (id: string) => CONTENTS.find((content) => content.id === id)!;
const versions = (...names: string[]) => ({ root: "/media/archive", versions: names.map((version) => ({ version, bytes: 1, time: "" })), lock: null, error: null });
const source = (id: string, extra: Partial<SourceRow> = {}) => ({ id, credential_missing: false, prepared: null, ...extra }) as SourceRow;
const lucerne = { west: 8.2, south: 46.9, east: 8.4, north: 47.1 };
const swiss = [5.9, 45.8, 10.5, 47.9] as [number, number, number, number];

const canopy = source("canopy-ch", { extent: swiss });

it("a card that needs a reference cannot bake until a national model meets the area and a version exists", () => {
    const forest = (...names: string[]) => [source("forest", { extent: [-180, -90, 180, 90], prepared: versions(...names) }), canopy];
    expect(needs(card("trailview-canopy"), lucerne, forest())).toMatchObject({ ready: false, reference: { source: "forest", version: null } });
    expect(needs(card("trailview-canopy"), lucerne, forest("2026-10-24")).ready).toBe(true);
    expect(needs(card("trailview-canopy"), { west: 11, south: 48, east: 12, north: 49 }, forest("2026-10-24"))).toMatchObject({ ready: false, models: [] });
    const everywhere = source("canopy-global", { extent: [-180, -85, 180, 85] });
    expect(needs(card("trailview-canopy"), { west: 11, south: 48, east: 12, north: 49 }, [...forest("2026-10-24"), everywhere])).toMatchObject({ ready: true, models: [everywhere] });
});

it("terrain falls back without a reference, the newest version of the archive serves, climate needs a version, and a missing login blocks", () => {
    const terrain = [source("terrain", { extent: swiss, prepared: versions("2026-10-01", "2026-10-24") })];
    expect(needs(TERRAIN, lucerne, terrain)).toMatchObject({ ready: true, reference: { version: { version: "2026-10-24" } } });
    expect(needs(TERRAIN, lucerne, []).ready).toBe(true);
    const climate = (...names: string[]) => [source("climate", { prepared: versions(...names) })];
    expect(needs(card("climate"), lucerne, climate())).toMatchObject({ ready: false, credentials: [], reference: { source: "climate", version: null } });
    expect(needs(card("climate"), lucerne, climate("2026-10-24")).ready).toBe(true);
    expect(needs(card("snow"), lucerne, [source("snow", { prepared: versions() })])).toMatchObject({ ready: false, reference: { source: "snow", version: null } });
    expect(needs(card("trailview"), lucerne, [source("sentinel-2-mosaics", { credential_missing: true })])).toMatchObject({ ready: false, credentials: ["sentinel-2-mosaics"] });
});

it("the flags switch exactly the layers of the cards that are on, and off the others", () => {
    expect(layerFlags(["map", "snow"]).filter(([flag]) => flag === "--add").map(([, layer]) => layer)).toEqual(["map", "snow"]);
    expect(layerFlags(["trailview", "snow"]).filter(([flag]) => flag === "--add").map(([, layer]) => layer)).toContain("trailview-snow");
    const flags = layerFlags(["trailview", "climate"]);
    expect(flags.filter(([flag]) => flag === "--add").map(([, layer]) => layer)).toEqual([
        "trailview-features",
        "trailview-rules",
        "trailview-ground",
        "trailview-colour",
        "climate",
    ]);
    expect(flags.filter(([flag]) => flag === "--remove").map(([, layer]) => layer)).toEqual([
        "device",
        "map",
        "routing",
        "search",
        "trailview-canopy",
        "snow",
        "trailview-snow",
    ]);
});

it("a card needs a card that shows it, and goes when the last one goes", () => {
    expect(unshown(card("climate"), ["routing"])).toBe("Shows in Planner map: switch it on first.");
    expect(unshown(card("snow"), ["trailview"])).toBeNull();
    expect(toggled(["map", "trailview", "climate", "snow", "trailview-canopy"], "map", false)).toEqual(["trailview", "snow", "trailview-canopy"]);
    expect(toggled(["trailview", "snow", "trailview-canopy"], "trailview", false)).toEqual([]);
});

it("an area that names its countries meets only their national models", () => {
    const bavaria = [source("terrain", { prepared: versions() }), source("dtm-de-by", { extent: [8.9, 47.2, 13.9, 50.6] })];
    expect(needs(TERRAIN, lucerne, bavaria, ["CH"]).models).toEqual([]);
    expect(needs(TERRAIN, { west: 8.5, south: 47.1, east: 9.2, north: 47.5 }, bavaria).models).toHaveLength(1);
});
