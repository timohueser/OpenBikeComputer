import { EXTENT, type Feature } from '../stream/features';
import { metresPerUnit } from '../terrain/bins';

/** World units per tile unit of a zoom 14 feature tile. */
const UNITS_PER_TILE_UNIT = 2 ** 30 / 2 ** 14 / EXTENT;
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

/**
 * The trees of the orchards of a zoom 14 feature tile: rows ROW_M apart along the longest side of
 * each area, a tree every STEP_M, inside the area and out of its holes. Only the trees inside the tile
 * itself, so neighbouring tiles that share an orchard in their buffers each give its trees once. Each
 * tree is x, y in whole world units.
 */
export function orchardRows(features: Feature[], tile: { x: number; y: number }): Float64Array {
    const out: number[] = [], left = tile.x * EXTENT, top = tile.y * EXTENT;
    for (const feature of features) {
        if (feature.kind !== 'orchard' || feature.geometry !== 'area' || BUSHES.has(feature.tags.produce)) continue;
        for (let p = 0; p < feature.parts.length; p++) {
            const part = feature.parts[p];
            if (!part.outer || part.xy.length < 6) continue;
            const holes = [];
            for (let h = p + 1; h < feature.parts.length && !feature.parts[h].outer; h++) holes.push(feature.parts[h]);
            // Metres from the first vertex, y north, turned so the longest side runs along x.
            const x0 = part.xy[0], y0 = part.xy[1], m = metresPerUnit((top + y0) * UNITS_PER_TILE_UNIT) * UNITS_PER_TILE_UNIT;
            const metres = (xy: Int32Array) => Array.from({ length: xy.length / 2 }, (_, i) => [(xy[2 * i] - x0) * m, -(xy[2 * i + 1] - y0) * m]);
            const outer = metres(part.xy);
            let angle = 0, longest = 0;
            outer.forEach((a, i) => {
                const b = outer[(i + 1) % outer.length], length = Math.hypot(b[0] - a[0], b[1] - a[1]);
                if (length > longest) [longest, angle] = [length, Math.atan2(b[1] - a[1], b[0] - a[0])];
            });
            const c = Math.cos(angle), s = Math.sin(angle);
            const turn = (ring: number[][]) => ring.map(([x, y]) => [c * x + s * y, -s * x + c * y]);
            const ring = turn(outer), rings = holes.map(hole => turn(metres(hole.xy)));
            const xs = ring.map(v => v[0]), ys = ring.map(v => v[1]);
            for (let v = Math.min(...ys) + ROW_M / 2; v < Math.max(...ys); v += ROW_M) {
                for (let u = Math.min(...xs) + STEP_M / 2; u < Math.max(...xs); u += STEP_M) {
                    if (!inside([u, v], ring) || rings.some(hole => inside([u, v], hole))) continue;
                    const tx = x0 + (c * u - s * v) / m, ty = y0 - (s * u + c * v) / m;
                    if (tx < 0 || ty < 0 || tx >= EXTENT || ty >= EXTENT) continue;
                    out.push(Math.round((left + tx) * UNITS_PER_TILE_UNIT), Math.round((top + ty) * UNITS_PER_TILE_UNIT));
                }
            }
        }
    }
    return Float64Array.from(out);
}
