import { expect, it } from "vitest";
import { leafSquare } from "./leaf";

it("a leaf is the square of 2^23 µdeg at its indices, as obc_osm::LeafId::square gives it", () => {
    expect(leafSquare("0037-0032")).toEqual([
        [41.94304, 0],
        [50.331648, 8.388608],
    ]);
    expect(leafSquare("device/map")).toBeNull();
});
