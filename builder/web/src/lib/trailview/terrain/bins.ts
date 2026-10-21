// The stamp data of a terrain node from the feature tile above it: the segments, lake edges and
// junctions near the node, binned into a grid of cells, as stamp.wgsl reads them. A worker uses it.
import { EARTH_RADIUS } from '../frame/frame';
import { EXTENT, type Feature } from '../stream/features';
import { PAGE } from '../stream/heights';
import CLASS_RULES from '../../../../../../data/trailview/roads.json' with { type: 'json' };

/** Words of a primitive record. */
export const RECORD = 16;
/** Cells along a node edge, with one cell of apron on each side; CELL texels each. */
export const GRID = 18;
export const CELL = PAGE / (GRID - 2);
/** Words before the cell offsets, and the offsets: GRID² + 1. */
const HEAD = 2;
const OFFSETS = GRID * GRID + 1;
/** A cell entry of a lake marker: the lake's cell centre lies inside it. */
export const INSIDE = 1 << 30;
/** Primitive kinds, as in stamp-core.wgsl. */
export const ROAD = 1, PATH = 2, EDGE = 3, LAKE = 4, JUNCTION = 5, RIVER = 6;
/** Road flags: a ford, and the round caps of the core and the cone at the start and the end. */
export const FORD = 1, CORE_A = 2, CORE_B = 4, CONE_A = 8, CONE_B = 16;
/** No stamp reaches further than the feature tiles' buffer, so a primitive goes to each cell within it. */
const REACH_M = 40;
/** The levels whose pages get the feature mask: they bin every road, also one that their level cannot show. */
export const NEAR = 16;
/** The lowest feature zoom, and the deepest. */
const LOW = 8, DEEP = 14;
const UNITS = 2 ** 30;

/** The surfaces in the order of the mask's surface code (index + 1) and their half-widths without a `width` tag, from the prototype. */
export const SURFACES = { main: 4.5, road: 3.0, gravel: 1.75, dirt: 1.5, grass: 1.25, path: 0.6 } as const;
type Surface = keyof typeof SURFACES;
const PAVED: Record<string, number> = { motorway: 6, trunk: 5, primary: 4, secondary: 3.25, tertiary: 3, unclassified: 2.5, residential: 2.5, service: 2, living_street: 2 };
const MAIN = ['motorway', 'trunk', 'primary', 'secondary', 'motorway_link', 'trunk_link', 'primary_link'];
const PATHS = ['path', 'footway', 'bridleway', 'steps', 'pedestrian', 'via_ferrata'];
/**
 * The profile of a road or path class, as `Profile` in stamp-core.wgsl. The core holds the road height
 * ± `tol`. The cut and the fill (rise per metre) widen until they stop binding `reach` metres past the
 * core, or for a path past its edge: its bench. A level stamps a road only where
 * `least` metres of its cut and fill fit inside the buffer. A path's bench may follow the hillside
 * across it by `tilt` metres per metre.
 */
export interface Profile { tol: number; reach: number; least: number; cut: number; fill: number; tilt: number }
/** One row per `highway` class of the road and path kinds, from the class rules that the road solver of the bake reads too. */
export const PROFILES: Record<string, Profile> = Object.fromEntries(Object.entries(CLASS_RULES).map(([name, { tol, reach, least, cut, fill, tilt }]) => [name, { tol, reach, least, cut, fill, tilt }]));
const CLASSES = Object.keys(PROFILES);
/** The row of a line's class; a class that the table lacks takes the row of its kind. */
const profileOf = (kind: string, highway = '') => CLASSES.indexOf(Object.hasOwn(PROFILES, highway) ? highway : kind === 'path' ? 'path' : 'road');
/** The table as stamp-core.wgsl reads it: PROFILE_ROWS rows of 8 floats. */
export const PROFILE_ROWS = 32, PROFILE_FLOATS = 8;
export function profileTable(): Float32Array<ArrayBuffer> {
    const rows = Object.values(PROFILES), table = new Float32Array(PROFILE_ROWS * PROFILE_FLOATS);
    if (rows.length > PROFILE_ROWS) throw new Error(`stamp-core.wgsl holds ${PROFILE_ROWS} profiles, not ${rows.length}.`);
    rows.forEach(({ tol, reach, least, cut, fill, tilt }, i) => table.set([tol, reach, least, cut, fill, tilt], PROFILE_FLOATS * i));
    return table;
}
/** The half-width of a waterway without a `width` tag. */
const WATERWAYS: Record<string, number> = { river: 5, canal: 5, stream: 1 };

export interface Node { level: number; x: number; y: number }

/**
 * The feature tile that a node reads: the zoom above its own level, as a terrain tile holds four
 * nodes; none above zoom 8.
 * which holds every kind at the deepest stored level.
 */
export function featureTile({ level, x, y }: Node): Node | undefined {
    const z = Math.min(level - 1, DEEP);
    return z < LOW ? undefined : { level: z, x: x >> (level - z), y: y >> (level - z) };
}

/** The prototype's road surface of a highway (`road_surface` in its bake). */
export function surfaceOf(tags: Record<string, string>): Surface {
    const { highway = '', surface = '', tracktype = '' } = tags;
    if (['asphalt', 'paved', 'concrete', 'paving_stones', 'sett', 'chipseal'].includes(surface)) return MAIN.slice(0, 4).includes(highway) ? 'main' : 'road';
    if (['gravel', 'compacted', 'fine_gravel', 'pebblestone'].includes(surface)) return 'gravel';
    if (['dirt', 'ground', 'earth', 'mud', 'unpaved', 'sand'].includes(surface)) return 'dirt';
    if (['grass', 'grass_paver'].includes(surface)) return 'grass';
    if (PATHS.includes(highway)) return 'path';
    if (highway === 'track') return ({ grade1: 'road', grade2: 'gravel', grade3: 'dirt' } as const)[tracktype as 'grade1'] ?? 'grass';
    return MAIN.includes(highway) ? 'main' : 'road';
}

/** The half-width of a road: OSM's `width` where it has one (`road_width` in the prototype). */
export function halfWidth(tags: Record<string, string>, surface: Surface): number {
    const width = parseFloat(tags.width ?? '');
    if (Number.isFinite(width)) return Math.max(width / 2, 0.4);
    return (surface === 'main' || surface === 'road') && PAVED[tags.highway] ? PAVED[tags.highway] : SURFACES[surface];
}

/** Metres per world unit at world row y, on the sphere of the frame. */
export const metresPerUnit = (y: number) => 2 * Math.PI * EARTH_RADIUS / UNITS / Math.cosh(Math.PI * (1 - 2 * y / UNITS));

/**
 * A line; `anchors` marks its OSM vertices among the points of its rounded bends. `plane` holds, per
 * vertex of a road, the gradient (metres per metre along world x and y) that its core takes across it:
 * the through road's surface near a junction, zero elsewhere.
 */
export interface Line { kind: number; xy: number[]; z: number[]; h: number; code: number; ford: boolean; profile: number; anchors?: boolean[]; plane?: number[] }
/** A lake: its level, and its rings in world units; a ring after an outer ring and before the next is its hole. */
export interface Lake { level: number; rings: number[][]; outer: boolean[] }

/**
 * As road_shows, path_shows and river_shows of stamp-core.wgsl: whether a level with texels of this
 * size can stamp a line. bins.test.ts holds these numbers equal to the shader's.
 */
export const RULES = { BUFFER_M: 40, MESH_TEXELS: 4, GATE_COS: 0.70710678, BANK_M: 2, BANK_RAMP: 8 } as const;
/** As MASK_RANGE of stamp.wgsl: the mask reads a road this far past its edge. */
const MASK_RANGE_M = 12.75;

/** As gate_texel of stamp-core.wgsl: the texel of a level at 45° N, for the gates that decide what a level stamps. */
export const gateTexel = (level: number) => 2 * Math.PI * EARTH_RADIUS * RULES.GATE_COS / 2 ** (level + 8);

export function shows({ kind, h, profile }: Line, texel: number): boolean {
    const limit = RULES.BUFFER_M - 2 * texel - 1, { reach, least } = Object.values(PROFILES)[profile];
    if (kind === PATH) return 2 * h >= texel && h + reach <= limit;
    if (kind === RIVER) return 2 * h >= texel && Math.max(h, RULES.MESH_TEXELS * texel) + RULES.BANK_M + 2 * RULES.MESH_TEXELS * texel + RULES.BANK_RAMP <= limit;
    return 2 * h >= texel && h + least <= limit;
}

/** The roads, paths, rivers and lakes of a tile that have heights, in world units. */
export function features(list: Feature[], tile: Node): { lines: Line[]; lakes: Lake[] } {
    const scale = 2 ** (17 - tile.level), ox = tile.x * EXTENT, oy = tile.y * EXTENT;
    const world = (xy: Int32Array, v: number) => [(ox + xy[2 * v]) * scale, (oy + xy[2 * v + 1]) * scale];
    const lines: Line[] = [], lakes: Lake[] = [];
    for (const { kind, tags, parts } of list) {
        if (kind === 'lake') {
            const level = parts[0]?.z?.[0];
            if (level === undefined || Number.isNaN(level)) continue;
            lakes.push({ level, rings: parts.map(({ xy }) => Array.from({ length: xy.length / 2 }, (_, v) => world(xy, v)).flat()), outer: parts.map(part => part.outer) });
            continue;
        }
        const river = kind === 'river' || kind === 'stream';
        if (!river && (!['main-road', 'road', 'path'].includes(kind) || (tags.bridge && tags.bridge !== 'no'))) continue;
        const surface = surfaceOf(tags), width = parseFloat(tags.width ?? '');
        const h = river ? (Number.isFinite(width) ? width / 2 : WATERWAYS[tags.waterway] ?? 1) : halfWidth(tags, surface), code = river ? 0 : Object.keys(SURFACES).indexOf(surface) + 1;
        const profile = river ? 0 : profileOf(kind, tags.highway);
        for (const { xy, z } of parts) {
            if (!z) continue;
            // A line splits where a vertex has no height: a missing height is unknown, never 0 m.
            let run: number[] = [], heights: number[] = [];
            const flush = () => {
                if (heights.length >= 2) lines.push({ kind: river ? RIVER : kind === 'path' ? PATH : ROAD, ...smoothed(run, heights), h, code, ford: !!tags.ford && tags.ford !== 'no', profile });
                run = [];
                heights = [];
            };
            for (let v = 0; v < z.length; v++) {
                if (Number.isNaN(z[v])) flush();
                else {
                    run.push(...world(xy, v));
                    heights.push(z[v]);
                }
            }
            flush();
        }
    }
    junctionPlanes(lines);
    return { lines, lakes };
}

/**
 * As APPROACH_M of tools/trailview/roads.py: the solver puts the first metres of a side road on the
 * through road's surface. Its cross-slope holds that far, or to the through road's edge, and goes over WARP_M.
 */
const APPROACH_M = 4, WARP_M = 6;

/**
 * The plane of the through road at each junction of roads, on the side roads. At a junction the
 * through road is the first class of roads.json there, and of two ways of that class, the pair that
 * runs most nearly straight on, as in the road solver of the bake; the solver puts each side road's
 * first metres on that road's surface along its middle. Its core takes the surface's slope across it
 * as far, or up to the through road's edge, and loses it over WARP_M beyond. Only OSM vertices and their heights
 * decide, so every tile that holds a junction gets the same planes.
 */
function junctionPlanes(lines: Line[]): void {
    const at = new Map<string, { line: Line; v: number }[]>();
    for (const line of lines) {
        if (line.kind !== ROAD) continue;
        line.plane = new Array(2 * line.z.length).fill(0);
        line.z.forEach((_, v) => {
            if (!(line.anchors?.[v] ?? true)) return;
            const name = `${line.xy[2 * v]},${line.xy[2 * v + 1]}`;
            at.set(name, [...(at.get(name) ?? []), { line, v }]);
        });
    }
    const next = (line: Line, v: number, step: number) => {
        let w = v + step;
        while (w > 0 && w < line.z.length - 1 && !(line.anchors?.[w] ?? true)) w += step;
        return w >= 0 && w < line.z.length ? w : -1;
    };
    const metres = (line: Line, a: number, b: number) => Math.hypot(line.xy[2 * b] - line.xy[2 * a], line.xy[2 * b + 1] - line.xy[2 * a + 1]) * metresPerUnit(line.xy[2 * a + 1]);
    for (const meeting of at.values()) {
        if (meeting.length < 2) continue;
        const ways = meeting.flatMap(({ line, v }) => [-1, 1].map(step => ({ line, v, step, w: next(line, v, step) })).filter(({ w }) => w >= 0)).map(way => {
            const { line, v, w } = way, length = Math.hypot(line.xy[2 * w] - line.xy[2 * v], line.xy[2 * w + 1] - line.xy[2 * v + 1]) || 1;
            return { ...way, dir: [(line.xy[2 * w] - line.xy[2 * v]) / length, (line.xy[2 * w + 1] - line.xy[2 * v + 1]) / length], grade: (line.z[w] - line.z[v]) / metres(line, v, w) };
        });
        if (ways.length < 2) continue;
        const best = Math.min(...ways.map(({ line }) => line.profile)), top = ways.filter(({ line }) => line.profile === best);
        let through = [top[0]];
        if (top.length >= 2) {
            let least = Infinity;
            for (let i = 0; i < top.length; i++) for (let k = i + 1; k < top.length; k++) {
                const dot = top[i].dir[0] * top[k].dir[0] + top[i].dir[1] * top[k].dir[1];
                if (dot < least) [least, through] = [dot, [top[i], top[k]]];
            }
        }
        const [axis] = through, grade = through.length === 2 ? (through[0].grade - through[1].grade) / 2 : axis.grade;
        const plane = [grade * axis.dir[0], grade * axis.dir[1]], full = Math.max(APPROACH_M, ...through.map(({ line }) => line.h));
        for (const side of ways) {
            if (through.includes(side)) continue;
            const { line, step } = side;
            let v = side.v, along = 0;
            while (v >= 0 && v < line.z.length) {
                const share = Math.min(1, Math.max(0, (full + WARP_M - along) / WARP_M));
                if (share * Math.hypot(...plane) > Math.hypot(line.plane![2 * v], line.plane![2 * v + 1])) line.plane!.splice(2 * v, 2, share * plane[0], share * plane[1]);
                if (share === 0 || v + step < 0 || v + step >= line.z.length) break;
                along += metres(line, v, v + step);
                v += step;
            }
        }
    }
}

/** The slopes across segment v of a road at its two ends: its plane on the segment's left normal, as `along` in stamp-core.wgsl reads them. */
export function crossSlopes(line: Line, v: number): [number, number] {
    if (!line.plane) return [0, 0];
    const dx = line.xy[2 * v + 2] - line.xy[2 * v], dy = line.xy[2 * v + 3] - line.xy[2 * v + 1], length = Math.hypot(dx, dy) || 1;
    const [nx, ny] = [-dy / length, dx / length];
    return [line.plane[2 * v] * nx + line.plane[2 * v + 1] * ny, line.plane[2 * v + 2] * nx + line.plane[2 * v + 3] * ny];
}

/** The most that a chord of a rounded bend leaves the curve, in world units (about 10 cm), and the most chords per segment. */
const SAGITTA = 4, CHORDS = 16;

/**
 * A line through its vertices with its bends rounded: each segment between two other segments follows
 * the centripetal Catmull-Rom curve through the four vertices about it, in chords of whole world
 * units, with the height linear along it. The first and last segments stay straight, also of a
 * closed way: the feature tiles cut a ring into open pieces, so every line is open. They cut a line
 * one vertex past the segments that meet a tile's buffer, so every segment that a tile's nodes stamp
 * gets the same curve in each tile that holds it; and every vertex stays, so junctions stay where they are.
 */
export function smoothed(xy: number[], z: number[]): { xy: number[]; z: number[]; anchors: boolean[] } {
    const n = z.length, segments = n - 1;
    if (n < 3) return { xy, z, anchors: z.map(() => true) };
    const point = (i: number): [number, number] => [xy[2 * i], xy[2 * i + 1]];
    const outXy = [xy[0], xy[1]], outZ = [z[0]], anchors = [true];
    for (let i = 0; i < segments; i++) {
        const [b, c] = [point(i), point(i + 1)];
        if (i > 0 && i < segments - 1) {
            const [a, d] = [point(i - 1), point(i + 2)], curve = catmullRom(a, b, c, d), middle = curve(0.5);
            const sagitta = Math.hypot(middle[0] - (b[0] + c[0]) / 2, middle[1] - (b[1] + c[1]) / 2);
            const chords = Math.min(CHORDS, Math.max(1, Math.ceil(Math.sqrt(sagitta / SAGITTA))));
            for (let k = 1; k < chords; k++) {
                const [x, y] = curve(k / chords).map(Math.round);
                if (x === outXy[outXy.length - 2] && y === outXy[outXy.length - 1]) continue;
                outXy.push(x, y);
                outZ.push(z[i] + (z[i + 1] - z[i]) * k / chords);
                anchors.push(false);
            }
        }
        outXy.push(xy[2 * (i + 1)], xy[2 * (i + 1) + 1]);
        outZ.push(z[i + 1]);
        anchors.push(true);
    }
    return { xy: outXy, z: outZ, anchors };
}

/** The centripetal Catmull-Rom curve from b to c (Barry and Goldman), as a function of 0..1. */
function catmullRom(a: number[], b: number[], c: number[], d: number[]): (u: number) => number[] {
    const knot = (p: number[], q: number[]) => Math.max(Math.sqrt(Math.hypot(q[0] - p[0], q[1] - p[1])), 1e-6);
    const t1 = knot(a, b), t2 = t1 + knot(b, c), t3 = t2 + knot(c, d);
    const lerp = (p: number[], q: number[], from: number, to: number, t: number) => p.map((v, i) => v + (q[i] - v) * (t - from) / (to - from));
    return u => {
        const t = t1 + (t2 - t1) * u;
        const a1 = lerp(a, b, 0, t1, t), a2 = lerp(b, c, t1, t2, t), a3 = lerp(c, d, t2, t3, t);
        return lerp(lerp(a1, a2, 0, t2, t), lerp(a2, a3, t1, t3, t), t1, t2, t);
    };
}

/** Junctions: vertices that two road lines meet at, or that one line passes twice (a ring's start), with the widest half-width there. */
export function junctions(lines: Line[]): Map<string, { x: number; y: number; h: number }> {
    const seen = new Map<string, { x: number; y: number; h: number; count: number }>();
    for (const line of lines) {
        for (let v = 0; v < line.z.length; v++) {
            const x = line.xy[2 * v], y = line.xy[2 * v + 1], key = `${x},${y}`;
            const entry = seen.get(key) ?? { x, y, h: 0, count: 0 };
            entry.count++;
            entry.h = Math.max(entry.h, line.h);
            seen.set(key, entry);
        }
    }
    return new Map([...seen].filter(([, { count }]) => count > 1));
}

/**
 * The unit mitre normal at vertex v of a line: along the line at an end, the bisector of the two segments
 * inside. At an OSM vertex the segments run to the OSM vertices about it, not to the points of a rounded
 * bend: a tile rounds only the segments with a vertex on both sides, so this is the same in every tile.
 */
export function mitre(line: Line, v: number): [number, number] {
    const n = line.z.length, anchored = line.anchors?.[v], dir = (a: number, b: number) => {
        const dx = line.xy[2 * b] - line.xy[2 * a], dy = line.xy[2 * b + 1] - line.xy[2 * a + 1], l = Math.hypot(dx, dy) || 1;
        return [dx / l, dy / l];
    };
    const back = (from: number) => {
        let w = from - 1;
        while (anchored && w > 0 && !line.anchors![w]) w--;
        return w;
    };
    const ahead = (from: number) => {
        let w = from + 1;
        while (anchored && w < n - 1 && !line.anchors![w]) w++;
        return w;
    };
    const before = v > 0 ? dir(back(v), v) : undefined;
    const after = v < n - 1 ? dir(v, ahead(v)) : undefined;
    const [x, y] = before && after ? [before[0] + after[0], before[1] + after[1]] : (before ?? after)!;
    const l = Math.hypot(x, y);
    return l > 1e-9 ? [x / l, y / l] : (after ?? before)! as [number, number];
}

/** Whether a point lies inside rings of world coordinates, even-odd. */
export function insideRings(rings: number[][], x: number, y: number): boolean {
    let inside = false;
    for (const ring of rings) {
        for (let i = 0, j = ring.length - 2; i < ring.length; j = i, i += 2) {
            const [ax, ay, bx, by] = [ring[j], ring[j + 1], ring[i], ring[i + 1]];
            if ((ay > y) !== (by > y) && x < ax + (y - ay) * (bx - ax) / (by - ay)) inside = !inside;
        }
    }
    return inside;
}

/** What a node stamps: its packed data for stamp.wgsl, and what it holds. */
export interface NodeStamps {
    /** The words of stamp-core.wgsl: counts, cell offsets, cell entries, records. Null when nothing is near. */
    data: Uint32Array<ArrayBuffer> | null;
    /** A road, a path or a lake is near: the node may stamp heights. */
    heights: boolean;
}

/**
 * The stamp data of a node from the features of its feature tile. Every primitive goes to each cell
 * that it can act on, so the shader sees every primitive that can reach a texel. A cell keeps the
 * order of stamp-core.wgsl: a lake's edges before its marker, a line's segments together.
 */
export function nodeStamps(list: Feature[], tile: Node, node: Node): NodeStamps {
    const { lines, lakes } = features(list, tile);
    const shift = 22 - node.level, cell = CELL * 2 ** shift;
    const x0 = node.x * PAGE * 2 ** shift - cell, y0 = node.y * PAGE * 2 ** shift - cell, span = GRID * cell;
    // Units per metre grow toward the pole: the node's poleward edge bounds them.
    const units = 1.02 / metresPerUnit(y0 < UNITS / 2 ? y0 - span : y0 + 2 * span);
    const records: number[][] = [];
    const cells: number[][][] = Array.from({ length: 3 }, () => Array.from({ length: GRID * GRID }, () => []));
    /** The cells within `metres` of a box. */
    const within = (box: number[], metres: number) => {
        const reach = metres * units, out: number[] = [];
        const i0 = Math.max(0, Math.floor((box[0] - reach - x0) / cell)), i1 = Math.min(GRID - 1, Math.floor((box[2] + reach - x0) / cell));
        const j0 = Math.max(0, Math.floor((box[1] - reach - y0) / cell)), j1 = Math.min(GRID - 1, Math.floor((box[3] + reach - y0) / cell));
        for (let j = j0; j <= j1; j++) for (let i = i0; i <= i1; i++) out.push(j * GRID + i);
        return out;
    };
    const record = (words: number[]) => records.push(words) - 1;
    const floats = new Float32Array(1), ints = new Uint32Array(floats.buffer);
    const f = (value: number) => ((floats[0] = value), ints[0]);

    const joins = junctions(lines.filter(line => line.kind === ROAD));
    for (const { x, y, h } of joins.values()) {
        const touched = within([x, y, x, y], REACH_M);
        if (!touched.length) continue;
        const at = record([x, y, 0, 0, JUNCTION, 0, f(h)]);
        for (const c of touched) cells[0][c].push(at);
    }
    let heights = false;
    lines.forEach((line, id) => {
        const stamps = shows(line, gateTexel(node.level));
        if (!stamps && node.level < NEAR) return;
        // A path acts no farther than its bench, and on the near levels than the mask reads it.
        const reach = line.kind === PATH ? line.h + (node.level >= NEAR ? MASK_RANGE_M : Object.values(PROFILES)[line.profile].reach) : REACH_M;
        const n = line.z.length, end = (v: number) => v === 0 || v === n - 1;
        for (let v = 0; v + 1 < n; v++) {
            const [ax, ay, bx, by] = line.xy.slice(2 * v, 2 * v + 4), [mx, my] = mitre(line, v), [nx, ny] = mitre(line, v + 1);
            const dead = (w: number) => end(w) && !joins.has(`${line.xy[2 * w]},${line.xy[2 * w + 1]}`);
            const flags = (line.ford ? FORD : 0) | (dead(v) ? CORE_A : 0) | (dead(v + 1) ? CORE_B : 0) | (end(v) ? CONE_A : 0) | (end(v + 1) ? CONE_B : 0);
            const touched = within([Math.min(ax, bx), Math.min(ay, by), Math.max(ax, bx), Math.max(ay, by)], reach);
            if (!touched.length) continue;
            const [ca, cb] = crossSlopes(line, v);
            const at = record([ax, ay, bx, by, line.kind, line.code | (flags << 8) | (line.profile << 16), f(line.z[v]), f(line.z[v + 1]), f(line.h), f(mx), f(my), f(nx), f(ny), id, f(ca), f(cb)]);
            for (const c of touched) cells[2][c].push(at);
            heights ||= stamps;
        }
    });
    // Per lake near the node, its edges in each cell that they reach.
    const near = lakes.flatMap(lake => {
        const xs = lake.rings.flatMap(ring => ring.filter((_, i) => i % 2 === 0)), ys = lake.rings.flatMap(ring => ring.filter((_, i) => i % 2 === 1));
        const reach = REACH_M * units;
        if (Math.max(...xs) < x0 - reach || Math.min(...xs) > x0 + span + reach || Math.max(...ys) < y0 - reach || Math.min(...ys) > y0 + span + reach) return [];
        const edges = new Map<number, number[]>();
        for (const ring of lake.rings) for (let i = 0, j = ring.length - 2; i < ring.length; j = i, i += 2) {
            const [ax, ay, bx, by] = [ring[j], ring[j + 1], ring[i], ring[i + 1]];
            const touched = within([Math.min(ax, bx), Math.min(ay, by), Math.max(ax, bx), Math.max(ay, by)], REACH_M);
            if (!touched.length) continue;
            const at = record([ax, ay, bx, by, EDGE]);
            for (const c of touched) edges.set(c, [...(edges.get(c) ?? []), at]);
        }
        return [{ lake, edges }];
    });
    // The origin of the lake crossings of a cell: its centre, held inside the node, as stamp-core.wgsl
    // has it, and moved a few units off every lake edge of the cell, so that no rule of a point on a
    // shore line decides inside or outside.
    const nudges = new Uint32Array(GRID * GRID);
    const origin = (c: number): [number, number] => {
        const held = (v: number, base: number) => Math.min(Math.max(v, base + cell), base + cell + PAGE * 2 ** shift - 1);
        const centre = [held(x0 + (c % GRID + 0.5) * cell, x0), held(y0 + (Math.floor(c / GRID) + 0.5) * cell, y0)];
        const edges = near.flatMap(({ edges }) => edges.get(c) ?? []).map(r => records[r]);
        for (const [dx, dy] of NUDGES) {
            const [x, y] = [centre[0] + dx, centre[1] + dy];
            if (edges.every(([ax, ay, bx, by]) => segmentDistance(x, y, ax, ay, bx, by) >= 1)) {
                nudges[c] = (dx + 8) | ((dy + 8) << 4);
                return [x, y];
            }
        }
        throw new Error(`No origin off the shores in cell ${c} of node ${node.level}/${node.x}/${node.y}`);
    };
    const origins = Array.from({ length: GRID * GRID }, (_, c): [number, number] => (near.length ? origin(c) : [0, 0]));
    for (const { lake, edges } of near) {
        // A cell lists the lake after its edges, where an edge is near or where its origin lies inside it.
        const marker = record([0, 0, 0, 0, LAKE, 0, f(lake.level)]);
        for (let c = 0; c < GRID * GRID; c++) {
            const inside = insideRings(lake.rings, ...origins[c]);
            if (!inside && !edges.has(c)) continue;
            cells[1][c].push(...(edges.get(c) ?? []), marker | (inside ? INSIDE : 0));
        }
        heights = true;
    }
    if (!records.length || (!heights && node.level < NEAR)) return { data: null, heights: false };
    const entries = cells[0].map((_, c) => [...cells[0][c], ...cells[1][c], ...cells[2][c]]);
    const count = entries.reduce((sum, list) => sum + list.length, 0);
    const data = new Uint32Array(HEAD + OFFSETS + count + RECORD * records.length);
    data[0] = records.length;
    data[1] = count;
    let at = HEAD + OFFSETS;
    entries.forEach((list, c) => {
        data[HEAD + c] = (at - HEAD - OFFSETS) | (nudges[c] << 24);
        data.set(list, at);
        at += list.length;
    });
    data[HEAD + GRID * GRID] = count;
    records.forEach((words, r) => data.set(words.map(w => w >>> 0), HEAD + OFFSETS + count + RECORD * r));
    return { data, heights };
}

/** The moves of a cell's origin that the worker tries, in world units, as a signed nibble each. */
const NUDGES = [[0, 0], [1, 0], [0, 1], [1, 1], [2, 1], [1, 2], [-1, 2], [3, 1], [-2, 3], [3, -2], [4, 3], [-3, 4]];

function segmentDistance(x: number, y: number, ax: number, ay: number, bx: number, by: number): number {
    const dx = bx - ax, dy = by - ay, t = Math.min(1, Math.max(0, ((x - ax) * dx + (y - ay) * dy) / (dx * dx + dy * dy || 1)));
    return Math.hypot(x - ax - t * dx, y - ay - t * dy);
}
