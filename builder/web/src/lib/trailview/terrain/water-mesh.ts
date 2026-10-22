// The water mesh of a node: each lake that reaches the node's square cut to it and into triangles at
// its level, and each river and stream a ribbon along its line. The stamp worker makes it with the
// node's stamps; water.wgsl draws it on the node's square, a ribbon on the drawn surface.
import earcut from 'earcut';
import type { Feature } from '../stream/features';
import { PAGE } from '../stream/heights';
import { features, metresPerUnit, mitre, RIVER, RULES, STREAM, type Line, type Node } from './bins';

/** Words of a vertex, as `WaterIn` of water.wgsl: x, y (int32 world units), level, across x and y, signed half-width. */
export const WATER_VERTEX = 6;
/** The longest edge: the linear drop between its ends leaves the curvature drop by at most 2 cm. */
export const EDGE_M = 1000;
/** A mitre grows at most this much in a sharp bend. */
const MITRE = 3;

export interface WaterMesh {
    vertices: ArrayBuffer;
    /** The ribbons' triangles, then the lakes'. */
    indices: Uint32Array<ArrayBuffer>;
    /** The indices of the ribbons. */
    ribbons: number;
}

/** The water mesh of a node from the features of its feature tile, null without water. */
export function waterMesh(list: Feature[], tile: Node, node: Node): WaterMesh | null {
    const { lines, lakes } = features(list, tile);
    const words: number[] = [], ribbons: number[] = [], surfaces: number[] = [];
    const vertex = (x: number, y: number, z: number, ax = 0, ay = 0, half = 0) => words.push(Math.round(x), Math.round(y), z, ax, ay, half) / WATER_VERTEX - 1;
    const shift = 22 - node.level, side = PAGE * 2 ** shift, x0 = node.x * side, y0 = node.y * side, margin = 2 ** shift;
    const box: [number, number, number, number] = [x0 - margin, y0 - margin, x0 + side + margin, y0 + side + margin];
    // A ribbon has a pair of vertices at least every mesh cell, so that it follows the mesh that it lies on.
    const longest = EDGE_M / metresPerUnit(y0 + side / 2), cell = RULES.MESH_TEXELS * 2 ** shift;

    const ribbon = (line: Line, run: number[]) => {
        const first = words.length / WATER_VERTEX;
        let pairs = 0;
        for (const [k, v] of run.entries()) {
            const last = k + 1 === run.length, [dx, dy] = direction(line.xy, last ? v - 1 : v), [tx, ty] = mitre(line, v);
            const [x, y, z] = [line.xy[2 * v], line.xy[2 * v + 1], line.z[v]];
            const [nx, ny, nz] = last ? [x, y, z] : [line.xy[2 * v + 2], line.xy[2 * v + 3], line.z[v + 1]];
            const parts = Math.ceil(Math.hypot(nx - x, ny - y) / cell) || 1;
            for (let p = 0; p < parts; p++, pairs++) {
                // At a vertex the ribbon takes the mitre; inside a segment it runs straight across it.
                const t = p / parts, [ux, uy] = p ? [dx, dy] : [tx, ty], scale = 1 / Math.max(ux * dx + uy * dy, 1 / MITRE);
                for (const sign of [1, -1]) vertex(x + (nx - x) * t, y + (ny - y) * t, z + (nz - z) * t, -sign * uy * scale, sign * ux * scale, sign * line.h);
            }
        }
        for (let i = 0; i + 1 < pairs; i++) ribbons.push(first + 2 * i, first + 2 * i + 1, first + 2 * i + 2, first + 2 * i + 2, first + 2 * i + 1, first + 2 * i + 3);
    };
    for (const line of lines) {
        if (line.kind !== RIVER && line.kind !== STREAM) continue;
        // Each run of segments that comes near the square is a ribbon.
        const reach = 2 * cell + line.h / metresPerUnit(y0 + side / 2);
        let run: number[] = [];
        for (let v = 0; v + 1 < line.z.length; v++) {
            const [ax, ay, bx, by] = line.xy.slice(2 * v, 2 * v + 4);
            if (Math.max(ax, bx) + reach < box[0] || Math.min(ax, bx) - reach > box[2] || Math.max(ay, by) + reach < box[1] || Math.min(ay, by) - reach > box[3]) {
                if (run.length) ribbon(line, run);
                run = [];
                continue;
            }
            if (!run.length) run.push(v);
            run.push(v + 1);
        }
        if (run.length) ribbon(line, run);
    }
    for (const { level, rings, outer } of lakes) {
        // Each outer ring with its holes, cut to the square, in triangles no longer than an edge may be.
        for (let r = 0; r < rings.length; r++) {
            const flat = outer[r] ? clipped(rings[r], box) : [], holes: number[] = [];
            if (flat.length < 6) continue;
            for (let k = r + 1; k < rings.length && !outer[k]; k++) {
                const hole = clipped(rings[k], box);
                if (hole.length < 6) continue;
                holes.push(flat.length / 2);
                flat.push(...hole);
            }
            const triangles = refined(flat, earcut(flat.map((v, i) => v - (i % 2 ? y0 : x0)), holes), longest);
            const first = words.length / WATER_VERTEX;
            for (let i = 0; i < flat.length; i += 2) vertex(flat[i], flat[i + 1], level);
            surfaces.push(...triangles.map(i => first + i));
        }
    }
    if (!ribbons.length && !surfaces.length) return null;
    const vertices = new ArrayBuffer(words.length * 4), floats = new Float32Array(vertices), ints = new Int32Array(vertices);
    words.forEach((word, i) => (i % WATER_VERTEX < 2 ? (ints[i] = word) : (floats[i] = word)));
    return { vertices, indices: new Uint32Array([...ribbons, ...surfaces]), ribbons: ribbons.length };
}

/** The unit direction of segment v of a line. */
function direction(xy: number[], v: number): [number, number] {
    const dx = xy[2 * v + 2] - xy[2 * v], dy = xy[2 * v + 3] - xy[2 * v + 1], l = Math.hypot(dx, dy) || 1;
    return [dx / l, dy / l];
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
