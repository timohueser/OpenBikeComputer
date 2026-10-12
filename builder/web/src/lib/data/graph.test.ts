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
