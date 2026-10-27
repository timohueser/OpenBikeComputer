// The samples of the invariant audit in a node, as audit.wgsl reads them: across each carriageway,
// in each lake and on its shore, along each river; and the checks of a tile's water levels. A worker uses it.
import type { Feature } from '../stream/features';
import { PAGE } from '../stream/heights';
import { BRIDGE, cores, crossSlope, features, insideRings, ringsDistance, junctions, metresPerUnit, mitre, RAIL, RIVER, ROAD, RULES, STREAM, type Line, type Node } from './bins';

/** Words of a sample, and its kinds. */
export const SAMPLE = 12;
export const CROSS_FALL = 1, ROAD_HEIGHT = 2, IN_WATER = 3, ON_BANK = 4, SHORE_SIDE = 5, UNDER_DECK = 6;
/** Samples per node at most. */
const MOST = 8192;
/** No river or other lake may lie within 13 texels of a shore sample, which the mesh there may read. */
const DRY_TEXELS = 13;
/** The side of a shore is checked on a grid of this many points across a node, this many texels from a shore at most. */
const SIDE_GRID = 64, SIDE_TEXELS = 20;

/**
 * The road height at a point as stamp-core.wgsl gives it: in the mitre quad of the nearest segment
 * that holds it, within `reach` units of its line, tilted by its cross-slopes; NaN in none. `unit` is
 * metres per world unit there.
 */
export function roadHeight(line: Line, p: [number, number], reach: number, unit: number): number {
    let best = reach, z = NaN;
    for (let v = 0; v + 1 < line.z.length; v++) {
        const [ax, ay, bx, by] = line.xy.slice(2 * v, 2 * v + 4), [mx, my] = mitre(line, v), [nx, ny] = mitre(line, v + 1);
        const length = Math.hypot(bx - ax, by - ay), dx = (bx - ax) / (length || 1), dy = (by - ay) / (length || 1);
        const da = ((p[0] - ax) * mx + (p[1] - ay) * my) / Math.max(dx * mx + dy * my, 0.1), db = ((bx - p[0]) * nx + (by - p[1]) * ny) / Math.max(dx * nx + dy * ny, 0.1);
        const cross = ((bx - ax) * (p[1] - ay) - (by - ay) * (p[0] - ax)) / (length || 1), lateral = Math.abs(cross);
        if (da >= 0 && db >= 0 && lateral <= best) {
            const t = da / Math.max(da + db, 1e-6);
            best = lateral;
            z = line.z[v] + (line.z[v + 1] - line.z[v]) * t + cross * unit * crossSlope(line, v, da * unit, db * unit);
        }
    }
    return z;
}

/** World units from a point to a line, its ends included: the stamps hold a round cap there. */
function lineDistance({ xy }: Line, x: number, y: number): number {
    let best = Infinity;
    for (let i = 0; i + 3 < xy.length; i += 2) {
        const ax = xy[i], ay = xy[i + 1], dx = xy[i + 2] - ax, dy = xy[i + 3] - ay;
        const t = Math.min(1, Math.max(0, ((x - ax) * dx + (y - ay) * dy) / (dx * dx + dy * dy || 1)));
        best = Math.min(best, Math.hypot(x - ax - t * dx, y - ay - t * dy));
    }
    return best;
}

/**
 * The water levels of a tile: lakes, and lakes with more than one level; rivers, and rivers whose
 * level rises downstream by more than the centimetre of the tiles.
 */
export function waterLevels(list: Feature[], tile: Node): { lakes: number; mixed: number; rivers: number; rising: number } {
    const out = { lakes: 0, mixed: 0, rivers: 0, rising: 0 };
    for (const { kind, parts } of list) {
        const levels = new Set(parts.flatMap(({ z }) => (z ? Array.from(z).filter(Number.isFinite) : [])));
        if (kind !== 'lake' || !levels.size) continue;
        out.lakes++;
        if (levels.size > 1) out.mixed++;
    }
    for (const { kind, z } of features(list, tile).lines) {
        if (kind !== RIVER && kind !== STREAM) continue;
        out.rivers++;
        if (z.some((level, i) => i > 0 && level > z[i - 1] + 0.005)) out.rising++;
    }
    return out;
}

/** The audit samples of a node from the features of its feature tile: SAMPLE words each. */
export function auditSamples(list: Feature[], tile: Node, node: Node): Uint32Array<ArrayBuffer> {
    const { lines, lakes } = features(list, tile);
    const shift = 22 - node.level, side = PAGE * 2 ** shift;
    const x0 = node.x * side, y0 = node.y * side, unit = metresPerUnit(y0 + side / 2), texel = unit * 2 ** shift;
    // The band of a lake's ramp on the node's level, where audit.wgsl checks the shore.
    const band = RULES.SHORE_CELLS * RULES.MESH_TEXELS * texel;
    const inNode = (x: number, y: number) => x >= x0 && x < x0 + side && y >= y0 && y < y0 + side;
    const out: number[][] = [];
    const floats = new Float32Array(1), ints = new Uint32Array(floats.buffer);
    const f = (value: number) => ((floats[0] = value), ints[0]);
    const push = (x: number, y: number, kind: number, expected: number, half: number, shore: number, junction = Infinity, junctionHalf = 0, other: [number, number] = [0, 0], otherExpected = 0) =>
        out.push([Math.round(x), Math.round(y), kind, f(expected), f(half), f(shore), f(junction), f(junctionHalf), Math.round(other[0]), Math.round(other[1]), f(otherExpected), node.level]);

    const joins = [...junctions(cores(lines)).values()];
    const step = Math.max(0.25, 2 * texel) / unit;
    for (const line of lines) {
        if (line.kind !== ROAD && line.kind !== RAIL) continue;
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
                const [zl, zr, zc] = [left, right, centre].map(point => roadHeight(line, point, 2 * line.h / unit, unit));
                if (Number.isNaN(zl) || Number.isNaN(zr) || Number.isNaN(zc)) continue;
                push(...left, CROSS_FALL | (line.profile << 8), zl, line.h, 0, junction, junctionHalf, right, zr);
                push(...centre, ROAD_HEIGHT | (line.profile << 8), zc, line.h, 0, junction, junctionHalf);
            }
        }
    }
    // Under a deck: its middle and its edges, with the metres to the nearer abutment, and its underside.
    for (const line of lines) {
        if (line.kind !== BRIDGE || !line.abut) continue;
        const n = line.z.length, along = [0];
        for (let v = 1; v < n; v++) along.push(along[v - 1] + Math.hypot(line.xy[2 * v] - line.xy[2 * v - 2], line.xy[2 * v + 1] - line.xy[2 * v - 1]) * unit);
        for (let v = 0; v + 1 < n; v++) {
            const [ax, ay, bx, by] = line.xy.slice(2 * v, 2 * v + 4), length = Math.hypot(bx - ax, by - ay);
            if (!length) continue;
            const across = [-(by - ay) / length, (bx - ax) / length], half = 0.98 * line.h / unit, count = Math.ceil(length / step);
            for (let i = 0; i < count; i++) {
                const t = (i + 0.5) / count, cx = ax + t * (bx - ax), cy = ay + t * (by - ay), s = along[v] + t * (along[v + 1] - along[v]);
                if (!inNode(cx, cy)) continue;
                const abutment = Math.min(line.abut[0] ? s : Infinity, line.abut[1] ? along[n - 1] - s : Infinity);
                const under = line.z[v] + t * (line.z[v + 1] - line.z[v]) - line.deck!;
                for (const k of [-1, 0, 1]) push(cx + k * across[0] * half, cy + k * across[1] * half, UNDER_DECK | (line.profile << 8), under, line.h, abutment);
            }
        }
    }
    // A shore sample has no other water near: a river's mouth, another lake.
    const rivers = lines.filter(line => line.kind === RIVER), near = DRY_TEXELS * texel;
    const wet = (x: number, y: number, self: unknown) =>
        lakes.some(lake => lake !== self && (insideRings(lake.rings, x, y) || ringsDistance(lake.rings, x, y) * unit < near)) ||
        rivers.some(line => lineDistance(line, x, y) * unit < line.h + near);
    for (const lake of lakes) {
        const { level, rings } = lake;
        // Inside: a grid over the node, every point at its distance from the shore.
        const grid = side / 32;
        for (let y = y0 + grid / 2; y < y0 + side; y += grid) for (let x = x0 + grid / 2; x < x0 + side; x += grid) {
            if (insideRings(rings, Math.round(x), Math.round(y))) push(x, y, IN_WATER, level, 0, ringsDistance(rings, Math.round(x), Math.round(y)) * unit);
        }
        // The shore: points on both sides of each edge in the node, in the band of its ramp.
        for (const ring of rings) {
            for (let i = 0, j = ring.length - 2; i < ring.length; j = i, i += 2) {
                const mx = (ring[i] + ring[j]) / 2, my = (ring[i + 1] + ring[j + 1]) / 2, length = Math.hypot(ring[i] - ring[j], ring[i + 1] - ring[j + 1]);
                if (!length || !inNode(mx, my)) continue;
                const normal = [-(ring[i + 1] - ring[j + 1]) / length, (ring[i] - ring[j]) / length];
                for (const sign of [-1, 1]) for (const past of [0.25, 0.75, 1.25]) {
                    const d = past * band / unit, x = Math.round(mx + sign * normal[0] * d), y = Math.round(my + sign * normal[1] * d);
                    if (inNode(x, y) && !wet(x, y, lake)) push(x, y, insideRings(rings, x, y) ? IN_WATER : ON_BANK, level, 0, ringsDistance(rings, x, y) * unit);
                }
            }
        }
    }
    // The side of the shores: a finer grid near them, at the distance from the nearest shore, negative in a lake.
    const fine = side / SIDE_GRID, margin = SIDE_TEXELS * texel / unit;
    const boxes = lakes.map(({ rings }) => {
        const xs = rings.flatMap(ring => ring.filter((_, i) => i % 2 === 0)), ys = rings.flatMap(ring => ring.filter((_, i) => i % 2 === 1));
        return [Math.min(...xs) - margin, Math.min(...ys) - margin, Math.max(...xs) + margin, Math.max(...ys) + margin];
    });
    for (let y = y0 + fine / 2; y < y0 + side; y += fine) for (let x = x0 + fine / 2; x < x0 + side; x += fine) {
        const [px, py] = [Math.round(x), Math.round(y)], near = lakes.filter((_, i) => px >= boxes[i][0] && py >= boxes[i][1] && px <= boxes[i][2] && py <= boxes[i][3]);
        if (!near.length) continue;
        const d = Math.min(...near.map(({ rings }) => ringsDistance(rings, px, py))) * unit;
        if (d < SIDE_TEXELS * texel) push(px, py, SHORE_SIDE, 0, 0, near.some(({ rings }) => insideRings(rings, px, py)) ? -d : d);
    }
    // Rivers: the middle of the river, away from a lake's bed. Its level may leave the mesh by the
    // steepest grade of the segment and its neighbours across a cell.
    const grade = (line: Line, w: number) => {
        const [ax, ay, bx, by] = line.xy.slice(2 * w, 2 * w + 4);
        return w < 0 || w + 1 >= line.z.length ? 0 : Math.abs(line.z[w + 1] - line.z[w]) / (Math.hypot(bx - ax, by - ay) * unit || 1);
    };
    for (const line of rivers) {
        for (let v = 0; v + 1 < line.z.length; v++) {
            const [ax, ay, bx, by] = line.xy.slice(2 * v, 2 * v + 4), length = Math.hypot(bx - ax, by - ay);
            if (!length) continue;
            const count = Math.ceil(length / step), steepest = Math.max(grade(line, v - 1), grade(line, v), grade(line, v + 1));
            for (let i = 0; i < count; i++) {
                const t = (i + 0.5) / count, cx = Math.round(ax + t * (bx - ax)), cy = Math.round(ay + t * (by - ay)), z = roadHeight(line, [cx, cy], line.h / unit, unit);
                if (inNode(cx, cy) && !Number.isNaN(z) && !lakes.some(({ rings }) => insideRings(rings, cx, cy) || ringsDistance(rings, cx, cy) * unit < near)) push(cx, cy, IN_WATER | (line.profile << 8), z, line.h, 0, Infinity, 0, [0, 0], steepest);
            }
        }
    }
    const every = Math.ceil(out.length / MOST);
    const kept = out.filter((_, i) => i % every === 0);
    const data = new Uint32Array(kept.length * SAMPLE);
    kept.forEach((words, i) => data.set(words.map(w => w >>> 0), i * SAMPLE));
    return data;
}
