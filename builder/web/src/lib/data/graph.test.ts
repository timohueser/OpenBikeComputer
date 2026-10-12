import { expect, it } from "vitest";
import status from "../../../test-support/data/status.json";
import { bands, followers, sourcesOf } from "./graph";
import type { Status } from "./shapes";

const layers = (status as Status).targets.find(({ target }) => target === "prod")!.layers!;

it("lays out the 13 layers of prod in a band for each product, a column for each depth of reads", () => {
    const names = bands(layers).map(({ product, columns }) => [product, columns.map((column) => column.map((layer) => layer.layer))]);
    expect(names).toEqual([
        ["device", [["device/terrain", "device/landmarks"], ["device/map"], ["device/catalog"]]],
        [
            "planner",
            [
                ["planner/routing", "planner/basemap", "planner/search", "planner/terrain", "planner/climate", "planner/snow"],
                ["planner/places", "planner/sun"],
                ["planner/index"],
            ],
        ],
    ]);
    expect(followers(layers, "planner/search")).toEqual(["planner/places", "planner/index"]);
    expect(followers(layers, "planner/terrain")).toEqual(["planner/sun", "planner/index"]);
    expect(sourcesOf(layers, "device/map")).toEqual(["copernicus-glo30", "land-polygons", "osm", "terrain-reference"]);
});

it("ends on layers that read each other", () => {
    const layer = (name: string, reads: string[]) => ({ ...layers[0], layer: name, reads });
    const cycle = [layer("device/a", ["device/b", "osm"]), layer("device/b", ["device/a"])];
    expect(bands(cycle)[0].columns.flat().map(({ layer }) => layer)).toEqual(["device/b", "device/a"]);
    expect(sourcesOf(cycle, "device/b")).toEqual(["osm"]);
    expect(followers(cycle, "device/a")).toEqual(["device/b"]);
});
