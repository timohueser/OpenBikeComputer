// The stored terrain for the road worker: a rail or a wall stands by the ground beside a road, and the worker
// decides that once per station, so no vertex reads the ground for it. It reads the terrain under the stamps, which
// only limit it by a road's cut and fill, so a decision does not move with the pages that the eye holds.
import { imagePixels } from '../../planner/layers/terrain';
import { heightOf, PAGE } from '../stream/heights';
import { TILE_ZOOM } from '../stream/terrain';
import { size, type Node } from '../terrain/lod';

/** Metres at a world point, NaN where no tile knows it. */
export type GroundAt = (x: number, y: number) => number;
/** The body of stored tile z/x/y, undefined where it is absent. */
export type StoredBody = (z: number, x: number, y: number) => Promise<ArrayBuffer | undefined>;
export interface GroundBody { z: number; x: number; y: number; body: ArrayBuffer }
export interface GroundTile { z: number; x: number; y: number; heights: Float32Array }

/** Pixels along a stored tile's edge. */
const SIDE = 2 * PAGE;
/** World units that the stations of an owner read past its square, about 100 m in the Alps: past a closed gap, a ramp and a line's run-on. */
const MARGIN = 2 ** 12;
/** The coarsest zoom that a road reads: a coarser pixel holds no cut. */
const LEAST_ZOOM = 10;
/** Tiles that the worker keeps decoded: the most that the owners about an eye read. Each takes 1 MiB. */
const KEPT = 9;

/** The stored tiles that hold the ground about an owner, of the deepest zoom that has each place. */
export async function groundBodies(owner: Node, body: StoredBody): Promise<GroundBody[]> {
    const side = size(owner.level), shift = 30 - TILE_ZOOM, found = new Map<string, GroundBody>();
    const [x0, x1] = [(owner.x * side - MARGIN) >> shift, ((owner.x + 1) * side + MARGIN) >> shift], [y0, y1] = [(owner.y * side - MARGIN) >> shift, ((owner.y + 1) * side + MARGIN) >> shift];
    const walks: Promise<void>[] = [];
    for (let y = y0; y <= y1; y++) for (let x = x0; x <= x1; x++) walks.push((async () => {
        for (let z = TILE_ZOOM, up = 0; z >= LEAST_ZOOM; z--, up++) {
            // A tile that fails leaves its stations without details until the mesh builds again. A coarser tile would
            // change what stands beside what its neighbour holds.
            const data = await body(z, x >> up, y >> up).catch(() => null);
            if (data === null) return;
            if (!data) continue;
            found.set(`${z}/${x >> up}/${y >> up}`, { z, x: x >> up, y: y >> up, body: data });
            return;
        }
    })());
    await Promise.all(walks);
    return [...found.values()];
}

const decoded = new Map<string, Promise<GroundTile | undefined>>();

/** The heights of a tile, decoded once; a tile that fails leaves its stations without details, and decodes anew for the next owner. */
export function decodedGround({ z, x, y, body }: GroundBody): Promise<GroundTile | undefined> {
    const name = `${z}/${x}/${y}`, known = decoded.get(name) ?? imagePixels(new Uint8Array(body)).then(({ data }) => ({ z, x, y, heights: Float32Array.from({ length: SIDE * SIDE }, (_, p) => heightOf(data, 4 * p)) }), () => {
        decoded.delete(name);
        return undefined;
    });
    decoded.delete(name);
    decoded.set(name, known);
    for (const old of decoded.keys()) {
        if (decoded.size <= KEPT) break;
        decoded.delete(old);
    }
    return known;
}

/** Catmull-Rom weights of the four texels about a part t, as the stamps read the stored ground (pages.wgsl). */
const spline = (t: number) => [0.5 * (-t + 2 * t * t - t * t * t), 0.5 * (2 - 5 * t * t + 3 * t * t * t), 0.5 * (t + 4 * t * t - 3 * t * t * t), 0.5 * (-t * t + t * t * t)];

/** The ground of some tiles: the deepest tile that holds a point answers. Past a tile's edge its texels hold their edge's heights. */
export function groundAt(tiles: (GroundTile | undefined)[]): GroundAt {
    const deepest = tiles.filter(tile => tile !== undefined).sort((a, b) => b.z - a.z), edge = (i: number) => Math.max(0, Math.min(SIDE - 1, i));
    return (x, y) => {
        for (const { z, x: tx, y: ty, heights } of deepest) {
            const units = 2 ** (30 - z) / SIDE, u = x / units - tx * SIDE - 0.5, v = y / units - ty * SIDE - 0.5;
            if (u < -0.5 || v < -0.5 || u >= SIDE - 0.5 || v >= SIDE - 0.5) continue;
            const i = Math.floor(u), j = Math.floor(v), wx = spline(u - i), wy = spline(v - j);
            let sum = 0;
            for (let b = 0; b < 4; b++) for (let a = 0; a < 4; a++) sum += wx[a] * wy[b] * heights[edge(j + b - 1) * SIDE + edge(i + a - 1)];
            return sum;
        }
        return NaN;
    };
}
