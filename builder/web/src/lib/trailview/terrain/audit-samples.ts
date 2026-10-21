// The samples of the invariant audit in a node, as audit.wgsl reads them: across each carriageway,
// inside each lake and on its banks. A worker uses it.
import type { Feature } from '../stream/features';
import { PAGE } from '../stream/heights';
import { features, insideRings, junctions, metresPerUnit, mitre, ROAD, type Line, type Node } from './bins';

/** Words of a sample, and its kinds. */
export const SAMPLE = 12;
export const CROSS_FALL = 1, ROAD_HEIGHT = 2, IN_LAKE = 3, ON_BANK = 4;
/** Samples per node at most. */
const MOST = 8192;
/** Distances outside a shore at which banks are checked, in texels of the node and metres beyond. */
const BANK_TEXELS = 13, BANK_STEPS = [0.3, 1, 1.7];

/**
 * The road height at a point as stamp-core.wgsl gives it: in the mitre quad of the nearest segment
 * that holds it, within `reach` units of its line; NaN in none.
 */
export function roadHeight(line: Line, p: [number, number], reach: number): number {
    let best = reach, z = NaN;
    for (let v = 0; v + 1 < line.z.length; v++) {
        const [ax, ay, bx, by] = line.xy.slice(2 * v, 2 * v + 4), [mx, my] = mitre(line, v), [nx, ny] = mitre(line, v + 1);
        const length = Math.hypot(bx - ax, by - ay), dx = (bx - ax) / (length || 1), dy = (by - ay) / (length || 1);
        const da = ((p[0] - ax) * mx + (p[1] - ay) * my) / Math.max(dx * mx + dy * my, 0.1), db = ((bx - p[0]) * nx + (by - p[1]) * ny) / Math.max(dx * nx + dy * ny, 0.1);
        const lateral = Math.abs((bx - ax) * (p[1] - ay) - (by - ay) * (p[0] - ax)) / (length || 1);
        if (da >= 0 && db >= 0 && lateral <= best) {
            best = lateral;
            z = line.z[v] + (line.z[v + 1] - line.z[v]) * da / Math.max(da + db, 1e-6);
        }
    }
    return z;
}

/** The distance from a point to the nearest edge of rings, in the units of the points. */
function shoreDistance(rings: number[][], x: number, y: number): number {
    let best = Infinity;
    for (const ring of rings) {
        for (let i = 0, j = ring.length - 2; i < ring.length; j = i, i += 2) {
            const ax = ring[j], ay = ring[j + 1], dx = ring[i] - ax, dy = ring[i + 1] - ay;
            const t = Math.min(1, Math.max(0, ((x - ax) * dx + (y - ay) * dy) / (dx * dx + dy * dy || 1)));
            best = Math.min(best, Math.hypot(x - ax - t * dx, y - ay - t * dy));
        }
    }
    return best;
}

/** The audit samples of a node from the features of its feature tile: SAMPLE words each. */
export function auditSamples(list: Feature[], tile: Node, node: Node): Uint32Array<ArrayBuffer> {
    const { lines, lakes } = features(list, tile);
    const shift = 22 - node.level, side = PAGE * 2 ** shift;
    const x0 = node.x * side, y0 = node.y * side, unit = metresPerUnit(y0 + side / 2), texel = unit * 2 ** shift;
    const inNode = (x: number, y: number) => x >= x0 && x < x0 + side && y >= y0 && y < y0 + side;
    const out: number[][] = [];
    const floats = new Float32Array(1), ints = new Uint32Array(floats.buffer);
    const f = (value: number) => ((floats[0] = value), ints[0]);
    const push = (x: number, y: number, kind: number, expected: number, half: number, shore: number, junction = Infinity, junctionHalf = 0, other: [number, number] = [0, 0], otherExpected = 0) =>
        out.push([Math.round(x), Math.round(y), kind, f(expected), f(half), f(shore), f(junction), f(junctionHalf), Math.round(other[0]), Math.round(other[1]), f(otherExpected), 0]);

    const joins = [...junctions(lines).values()];
    const step = Math.max(0.25, 2 * texel) / unit;
    for (const line of lines) {
        if (line.kind !== ROAD) continue;
        for (let v = 0; v + 1 < line.z.length; v++) {
            const [ax, ay, bx, by] = line.xy.slice(2 * v, 2 * v + 4), length = Math.hypot(bx - ax, by - ay);
            if (!length) continue;
            const across = [-(by - ay) / length, (bx - ax) / length], half = 0.98 * line.h / unit, count = Math.ceil(length / step);
            for (let i = 0; i < count; i++) {
                const t = (i + 0.5) / count, cx = ax + t * (bx - ax), cy = ay + t * (by - ay);
                if (!inNode(cx, cy)) continue;
                let junction = Infinity, junctionHalf = 0;
                for (const j of joins) {
                    const d = Math.hypot(j.x - cx, j.y - cy) * unit;
                    if (d < junction) [junction, junctionHalf] = [d, j.h];
                }
                // Whole world units, as the shader reads them.
                const at = (x: number, y: number): [number, number] => [Math.round(x), Math.round(y)];
                const left = at(cx - across[0] * half, cy - across[1] * half), right = at(cx + across[0] * half, cy + across[1] * half), centre = at(cx, cy);
                const [zl, zr, zc] = [left, right, centre].map(point => roadHeight(line, point, 2 * line.h / unit));
                if (Number.isNaN(zl) || Number.isNaN(zr) || Number.isNaN(zc)) continue;
                push(...left, CROSS_FALL, zl, line.h, 0, junction, junctionHalf, right, zr);
                push(...centre, ROAD_HEIGHT, zc, line.h, 0, junction, junctionHalf);
            }
        }
    }
    for (const { level, rings } of lakes) {
        // Inside: a grid over the node, every point at its distance from the shore.
        const grid = side / 32;
        for (let y = y0 + grid / 2; y < y0 + side; y += grid) for (let x = x0 + grid / 2; x < x0 + side; x += grid) {
            if (insideRings(rings, Math.round(x), Math.round(y))) push(x, y, IN_LAKE, level, 0, shoreDistance(rings, Math.round(x), Math.round(y)) * unit);
        }
        // Banks: points outside each edge in the node, where the bank band begins.
        for (const ring of rings) {
            for (let i = 0, j = ring.length - 2; i < ring.length; j = i, i += 2) {
                const mx = (ring[i] + ring[j]) / 2, my = (ring[i + 1] + ring[j + 1]) / 2, length = Math.hypot(ring[i] - ring[j], ring[i + 1] - ring[j + 1]);
                if (!length || !inNode(mx, my)) continue;
                const normal = [-(ring[i + 1] - ring[j + 1]) / length, (ring[i] - ring[j]) / length];
                for (const sign of [-1, 1]) for (const extra of BANK_STEPS) {
                    const d = (BANK_TEXELS * texel + extra) / unit, x = Math.round(mx + sign * normal[0] * d), y = Math.round(my + sign * normal[1] * d);
                    if (inNode(x, y) && !insideRings(rings, x, y)) push(x, y, ON_BANK, level, 0, shoreDistance(rings, x, y) * unit);
                }
            }
        }
    }
    const every = Math.ceil(out.length / MOST);
    const kept = out.filter((_, i) => i % every === 0);
    const data = new Uint32Array(kept.length * SAMPLE);
    kept.forEach((words, i) => data.set(words.map(w => w >>> 0), i * SAMPLE));
    return data;
}
