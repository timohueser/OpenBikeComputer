import { EXTENT, type Feature } from '../stream/features';
import { metresPerUnit } from '../terrain/bins';

/** World units per tile unit of a zoom 14 feature tile, and along a row of zoom 8 tiles. */
const UNITS_PER_TILE_UNIT = 2 ** 30 / 2 ** 14 / EXTENT, ROW_UNITS = 2 ** 30 / 2 ** 8;
/** Metres between the rows of an orchard and between its trees in a row: the high-stem orchards of the Alps' foreland. */
export const ROW_M = 10, STEP_M = 9;
/** Orchards of berries and bushes have no trees. */
const BUSHES = new Set(['blueberry', 'strawberry', 'raspberry', 'blackberry', 'currant', 'gooseberry']);

/** Whether a point lies in a ring of points, by the crossings of a ray along +x. */
function inside([x, y]: number[], ring: number[][]): boolean {
    let crossings = 0;
    ring.forEach((a, i) => {
        const b = ring[(i + 1) % ring.length];
        if (a[1] > y !== b[1] > y && a[0] + ((y - a[1]) * (b[0] - a[0])) / (b[1] - a[1]) > x) crossings++;
    });
    return crossings % 2 === 1;
}

/** Whether a vertex of a ring lies off the tile, in its buffer, where the buffer's edge may clip the ring. */
const off = (xy: Int32Array, i: number) => xy[2 * i] <= 0 || xy[2 * i] >= EXTENT || xy[2 * i + 1] <= 0 || xy[2 * i + 1] >= EXTENT;

/** Whether a side of a ring runs along the buffer's edge: no side of the orchard itself. */
function clipped(xy: Int32Array, i: number, j: number): boolean {
    const edge = (v: number) => v <= 0 || v >= EXTENT;
    return (xy[2 * i] === xy[2 * j] && edge(xy[2 * i])) || (xy[2 * i + 1] === xy[2 * j + 1] && edge(xy[2 * i + 1]));
}

/**
 * The trees of the orchards of a zoom 14 feature tile: rows ROW_M apart along the longest side of
 * each area, a tree every STEP_M, inside the area and out of its holes. The rows lie on that side's
 * line and the trees on a lattice of the world along it, so the tiles that hold an orchard, each
 * clipped by its own buffer, lay the same trees; each gives only those inside the tile itself.
 * Orchards of berries or bushes (`trees=*_plants`) have none. Each tree is x, y in whole world units.
 */
export function orchardRows(features: Feature[], tile: { x: number; y: number }): Float64Array {
    const out: number[] = [], left = tile.x * EXTENT, top = tile.y * EXTENT;
    // Metres per world unit at the middle of the tile's row of zoom 8 tiles: the same for the tiles about it.
    const m = metresPerUnit((Math.floor(top * UNITS_PER_TILE_UNIT / ROW_UNITS) + 0.5) * ROW_UNITS);
    const world = (xy: Int32Array, i: number) => [(left + xy[2 * i]) * UNITS_PER_TILE_UNIT, (top + xy[2 * i + 1]) * UNITS_PER_TILE_UNIT];
    for (const feature of features) {
        if (feature.kind !== 'orchard' || feature.geometry !== 'area' || BUSHES.has(feature.tags.produce) || feature.tags.trees?.endsWith('_plants')) continue;
        for (let p = 0; p < feature.parts.length; p++) {
            const part = feature.parts[p], n = part.xy.length / 2;
            if (!part.outer || n < 3) continue;
            const holes = [];
            for (let h = p + 1; h < feature.parts.length && !feature.parts[h].outer; h++) holes.push(feature.parts[h]);
            let side = -1, longest = 0;
            for (let i = 0; i < n; i++) {
                const j = (i + 1) % n, length = Math.hypot(part.xy[2 * j] - part.xy[2 * i], part.xy[2 * j + 1] - part.xy[2 * i + 1]);
                if (length > longest && !clipped(part.xy, i, j)) [side, longest] = [i, length];
            }
            if (side < 0) continue;
            // Along the longest side (u) and across it from its line (v), in metres; a corner of the orchard anchors the line.
            const next = (side + 1) % n, a = world(part.xy, off(part.xy, side) ? next : side), b = world(part.xy, next), o = world(part.xy, side);
            const length = Math.hypot(b[0] - o[0], b[1] - o[1]), dir = [(b[0] - o[0]) / length, (b[1] - o[1]) / length];
            const turned = (xy: Int32Array) => Array.from({ length: xy.length / 2 }, (_, i) => {
                const [x, y] = world(xy, i);
                return [(x * dir[0] + y * dir[1]) * m, ((x - a[0]) * -dir[1] + (y - a[1]) * dir[0]) * m];
            });
            const ring = turned(part.xy), rings = holes.map(hole => turned(hole.xy));
            const us = ring.map(v => v[0]), vs = ring.map(v => v[1]), along = a[0] * dir[0] + a[1] * dir[1];
            for (let v = (Math.floor(Math.min(...vs) / ROW_M) + 0.5) * ROW_M; v < Math.max(...vs); v += ROW_M) {
                for (let u = (Math.floor(Math.min(...us) / STEP_M) + 0.5) * STEP_M; u < Math.max(...us); u += STEP_M) {
                    if (!inside([u, v], ring) || rings.some(hole => inside([u, v], hole))) continue;
                    const x = a[0] + dir[0] * (u / m - along) - dir[1] * v / m, y = a[1] + dir[1] * (u / m - along) + dir[0] * v / m;
                    const tx = x / UNITS_PER_TILE_UNIT - left, ty = y / UNITS_PER_TILE_UNIT - top;
                    if (tx < 0 || ty < 0 || tx >= EXTENT || ty >= EXTENT) continue;
                    out.push(Math.round(x), Math.round(y));
                }
            }
        }
    }
    return Float64Array.from(out);
}
