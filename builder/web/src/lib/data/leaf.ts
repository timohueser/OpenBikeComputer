// A grid leaf `I-J` of a tiled layer is the square of 2^23 µdeg whose south-west corner is
// GRID_ORIGIN + (I, J)·2^23 (`obc_formats::grid`, `obc_osm::SOURCE_LEAF_LOG2`). A test of
// `obc_osm::LeafId::square` pins the numbers that leaf.test.ts uses.
import type { DegPoint } from "../map/coverageMap";

const [ORIGIN, SIDE] = [-(2 ** 28), 2 ** 23];

/** `[[south, west], [north, east]]` in degrees; `null` for an id that is not `I-J`. */
export function leafSquare(id: string): [DegPoint, DegPoint] | null {
    const match = /^(\d+)-(\d+)$/.exec(id);
    if (!match) return null;
    const [south, west] = [match[1], match[2]].map((n) => (ORIGIN + Number(n) * SIDE) / 1e6);
    return [
        [south, west],
        [south + SIDE / 1e6, west + SIDE / 1e6],
    ];
}
