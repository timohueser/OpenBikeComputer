// The water meshes of a feature tile: each lake cut into triangles at its level, with a skirt past
// its shore that goes under the banks, and each river a strip at its levels. A worker makes them;
// water.wgsl draws them for each drawn node that reads the tile, and widens the skirts and strips
// by the reach of the node's level.
import earcut from 'earcut';
import type { Feature } from '../stream/features';
import { features, insideRings, metresPerUnit, mitre, RIVER, type Node } from './bins';

/** Words of a vertex, as `WaterIn` of water.wgsl: x, y (int32 world units), level, normal x and y, base metres, reach share, river half-width. */
export const WATER_VERTEX = 8;
/** The longest edge: the linear drop between its ends leaves the curvature drop by at most 2 cm. */
export const EDGE_M = 1000;
/** A mitre normal grows at most this much in a sharp corner. */
const MITRE = 3;

export interface WaterMesh {
    vertices: ArrayBuffer;
    indices: Uint32Array<ArrayBuffer>;
}

/** The water meshes of a tile's lakes and rivers, null without water. */
export function waterMesh(list: Feature[], tile: Node): WaterMesh | null {
    const { lines, lakes } = features(list, tile);
    const words: number[] = [], indices: number[] = [];
    const vertex = (x: number, y: number, z: number, [nx, ny] = [0, 0], base = 0, reach = 0, half = 0) => words.push(Math.round(x), Math.round(y), z, nx, ny, base, reach, half) / WATER_VERTEX - 1;
    // Two triangles between the vertex pairs a, a + 1 and b, b + 1.
    const quad = (a: number, b: number) => indices.push(a, a + 1, b, b, a + 1, b + 1);
    const span = 2 ** (30 - tile.level), x0 = tile.x * span, y0 = tile.y * span, margin = span / 64;
    const box: [number, number, number, number] = [x0 - margin, y0 - margin, x0 + span + margin, y0 + span + margin];
    const longest = EDGE_M / metresPerUnit(y0 + span / 2);

    for (const { level, rings, outer } of lakes) {
        // A little shorter than `longest`, so that the rounding of a vertex never makes `refined` split a shore edge.
        const split = rings.map(ring => subdivided(ring, 0.95 * longest));
        // The skirt: each shore edge goes on outward, under the bank, by the reach of the level that draws it.
        for (const ring of split) {
            const n = ring.length / 2, side = outwardSide(ring, split), first = words.length / WATER_VERTEX;
            const normals = Array.from({ length: n }, (_, i) => edgeNormal(ring, i, (i + 1) % n, side));
            for (let i = 0; i < n; i++) {
                vertex(ring[2 * i], ring[2 * i + 1], level);
                vertex(ring[2 * i], ring[2 * i + 1], level, cornerNormal(normals[(i + n - 1) % n], normals[i]), 0, 1);
            }
            for (let i = 0; i < n; i++) quad(first + 2 * i, first + 2 * ((i + 1) % n));
        }
        // The surface: each outer ring with its holes, cut to the tile, in triangles no longer than an edge may be.
        for (let r = 0; r < split.length; r++) {
            const flat = outer[r] ? clipped(split[r], box) : [], holes: number[] = [];
            if (flat.length < 6) continue;
            for (let k = r + 1; k < split.length && !outer[k]; k++) {
                const hole = clipped(split[k], box);
                if (hole.length < 6) continue;
                holes.push(flat.length / 2);
                flat.push(...hole);
            }
            const triangles = refined(flat, earcut(flat.map((v, i) => v - (i % 2 ? y0 : x0)), holes), longest);
            const first = words.length / WATER_VERTEX;
            for (let i = 0; i < flat.length; i += 2) vertex(flat[i], flat[i + 1], level);
            indices.push(...triangles.map(i => first + i));
        }
    }
    // A river: two vertices across each vertex, at its half-width and the reach of the level that draws it.
    for (const line of lines) {
        if (line.kind !== RIVER) continue;
        const xy: number[] = [], z: number[] = [];
        for (let v = 0; v + 1 < line.z.length; v++) {
            const [ax, ay, bx, by] = line.xy.slice(2 * v, 2 * v + 4), parts = Math.ceil(Math.hypot(bx - ax, by - ay) / longest);
            for (let k = 0; k < parts; k++) {
                xy.push(Math.round(ax + (bx - ax) * k / parts), Math.round(ay + (by - ay) * k / parts));
                z.push(line.z[v] + (line.z[v + 1] - line.z[v]) * k / parts);
            }
        }
        xy.push(...line.xy.slice(-2));
        z.push(line.z[line.z.length - 1]);
        const sub = { ...line, xy, z, anchors: undefined }, first = words.length / WATER_VERTEX;
        for (let v = 0; v < z.length; v++) {
            const [tx, ty] = mitre(sub, v), w = Math.min(v, z.length - 2);
            const [ax, ay] = edgeNormal(xy, w, w + 1, -1), scale = 1 / Math.max(-ty * ax + tx * ay, 1 / MITRE);
            for (const sign of [1, -1]) vertex(xy[2 * v], xy[2 * v + 1], z[v], [-sign * ty * scale, sign * tx * scale], line.h, 1, line.h);
        }
        for (let v = 0; v + 1 < z.length; v++) quad(first + 2 * v, first + 2 * v + 2);
    }
    if (!indices.length) return null;
    const vertices = new ArrayBuffer(words.length * 4), floats = new Float32Array(vertices), ints = new Int32Array(vertices);
    words.forEach((word, i) => (i % WATER_VERTEX < 2 ? (ints[i] = word) : (floats[i] = word)));
    return { vertices, indices: new Uint32Array(indices) };
}

/** A ring with each edge split into equal parts no longer than `longest`. */
function subdivided(ring: number[], longest: number): number[] {
    const n = ring.length / 2, out: number[] = [];
    for (let i = 0; i < n; i++) {
        const j = (i + 1) % n, parts = Math.ceil(Math.hypot(ring[2 * j] - ring[2 * i], ring[2 * j + 1] - ring[2 * i + 1]) / longest);
        for (let k = 0; k < parts; k++) out.push(Math.round(ring[2 * i] + (ring[2 * j] - ring[2 * i]) * k / parts), Math.round(ring[2 * i + 1] + (ring[2 * j + 1] - ring[2 * i + 1]) * k / parts));
    }
    return out;
}

/** 1 when the water lies to the right of the ring's edges (x east, y down), else −1: edgeNormal with it points out of the water. */
function outwardSide(ring: number[], rings: number[][]): number {
    const n = ring.length / 2;
    for (let i = 0; i < n; i++) {
        const j = (i + 1) % n, [nx, ny] = edgeNormal(ring, i, j, 1), step = 0.01 * Math.hypot(ring[2 * j] - ring[2 * i], ring[2 * j + 1] - ring[2 * i + 1]);
        if (step > 0.5) return insideRings(rings, (ring[2 * i] + ring[2 * j]) / 2 + nx * step, (ring[2 * i + 1] + ring[2 * j + 1]) / 2 + ny * step) ? -1 : 1;
    }
    return 1;
}

/** The unit normal of edge i–j: to its left (x east, y down) times `side`. */
function edgeNormal(xy: number[], i: number, j: number, side: number): [number, number] {
    const dx = xy[2 * j] - xy[2 * i], dy = xy[2 * j + 1] - xy[2 * i + 1], l = Math.hypot(dx, dy) || 1;
    return [side * dy / l, -side * dx / l];
}

/** The normal at a corner between two edge normals, so that the offset edges stay parallel to the edges. */
function cornerNormal([ax, ay]: [number, number], [bx, by]: [number, number]): [number, number] {
    const x = ax + bx, y = ay + by, l = Math.hypot(x, y);
    if (l < 1e-9) return [ax, ay];
    const scale = 1 / Math.max((x * ax + y * ay) / l, 1 / MITRE);
    return [x / l * scale, y / l * scale];
}

/** A ring cut to a box, one side at a time (Sutherland–Hodgman). */
function clipped(ring: number[], box: [number, number, number, number]): number[] {
    let out = ring;
    for (const [axis, bound, sign] of [[0, box[0], 1], [0, box[2], -1], [1, box[1], 1], [1, box[3], -1]]) {
        const input = out, n = input.length / 2, inside = (i: number) => (input[2 * i + axis] - bound) * sign >= 0;
        out = [];
        for (let i = 0; i < n; i++) {
            const p = (i + n - 1) % n;
            if (inside(i) !== inside(p)) {
                const t = (bound - input[2 * p + axis]) / (input[2 * i + axis] - input[2 * p + axis]);
                out.push(input[2 * p] + t * (input[2 * i] - input[2 * p]), input[2 * p + 1] + t * (input[2 * i + 1] - input[2 * p + 1]));
            }
            if (inside(i)) out.push(input[2 * i], input[2 * i + 1]);
        }
    }
    return out;
}

/**
 * Triangles with each edge no longer than `longest`: the longest edge of a triangle splits at its
 * middle, which the triangle across that edge shares, so no vertex lies on another triangle's edge.
 */
function refined(xy: number[], triangles: number[], longest: number): number[] {
    const middles = new Map<string, number>(), out: number[] = [], stack: number[][] = [];
    const length = (a: number, b: number) => Math.hypot(xy[2 * a] - xy[2 * b], xy[2 * a + 1] - xy[2 * b + 1]);
    for (let i = 0; i < triangles.length; i += 3) stack.push(triangles.slice(i, i + 3));
    while (stack.length) {
        const [a, b, c] = stack.pop()!;
        const [p, q, r] = [[a, b, c], [b, c, a], [c, a, b]].reduce((best, edge) => (length(edge[0], edge[1]) > length(best[0], best[1]) ? edge : best));
        if (length(p, q) <= longest) {
            out.push(a, b, c);
            continue;
        }
        const name = p < q ? `${p},${q}` : `${q},${p}`;
        let m = middles.get(name);
        if (m === undefined) {
            m = xy.length / 2;
            xy.push(Math.round((xy[2 * p] + xy[2 * q]) / 2), Math.round((xy[2 * p + 1] + xy[2 * q + 1]) / 2));
            middles.set(name, m);
        }
        stack.push([p, m, r], [m, q, r]);
    }
    return out;
}
