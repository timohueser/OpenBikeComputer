// The stamp data of a terrain node from the feature tile above it: the segments, lake edges and
// junctions near the node, binned into a grid of cells, as stamp.wgsl reads them. A worker uses it.
import { EARTH_RADIUS } from '../frame/frame';
import { EXTENT, type Feature } from '../stream/features';
import { PAGE } from '../stream/heights';

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
/** The half-width of a waterway without a `width` tag. */
const WATERWAYS: Record<string, number> = { river: 5, canal: 5, stream: 1 };

export interface Node { level: number; x: number; y: number }

/**
 * The feature tile that a node reads: the zoom above its own level, as a terrain tile holds four
 * nodes; none above zoom 8. Conditioning baked into a stored level reads the tile of its own zoom,
 * which holds every kind at the deepest stored level.
 */
export function featureTile({ level, x, y }: Node, baked = false): Node | undefined {
    const z = Math.min(baked ? level : level - 1, DEEP);
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

export interface Line { kind: number; xy: number[]; z: number[]; h: number; code: number; ford: boolean; closed: boolean }
/** A lake: its level, and its rings in world units; a ring after an outer ring and before the next is its hole. */
export interface Lake { level: number; rings: number[][]; outer: boolean[] }

/**
 * As road_shows, path_shows and river_shows of stamp-core.wgsl: whether a level with texels of this
 * size can stamp a line. bins.test.ts holds these numbers equal to the shader's.
 */
export const RULES = { BUFFER_M: 40, SHOW_CELLS: 1, MESH_TEXELS: 4, MARGIN_TEXELS: 13, CONE_LEAST: 5, BENCH_REACH: 3, GATE_COS: 0.70710678 } as const;
/** As MASK_RANGE of stamp.wgsl: the mask reads a road this far past its edge. */
const MASK_RANGE_M = 12.75;

/** As gate_texel of stamp-core.wgsl: the texel of a level at 45° N, for the gates that decide what a level stamps. */
export const gateTexel = (level: number) => 2 * Math.PI * EARTH_RADIUS * RULES.GATE_COS / 2 ** (level + 8);

export function shows(kind: number, h: number, texel: number, baked: boolean): boolean {
    const limit = RULES.BUFFER_M - 2 * texel - 1;
    if (kind === PATH) return 2 * h >= texel && h + RULES.BENCH_REACH <= limit;
    if (kind === RIVER) return 2 * h >= RULES.MESH_TEXELS * texel && h <= limit;
    const margin = baked ? 0 : RULES.MARGIN_TEXELS * texel;
    return (baked || 2 * h >= RULES.SHOW_CELLS * RULES.MESH_TEXELS * texel) && h + margin + RULES.CONE_LEAST <= limit;
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
        for (const { xy, z } of parts) {
            if (!z) continue;
            // A line splits where a vertex has no height: a missing height is unknown, never 0 m.
            let run: number[] = [], heights: number[] = [];
            const flush = () => {
                if (heights.length >= 2) {
                    const n = heights.length, closed = n >= 3 && run[0] === run[2 * n - 2] && run[1] === run[2 * n - 1];
                    lines.push({ kind: river ? RIVER : kind === 'path' ? PATH : ROAD, xy: run, z: heights, h, code, ford: !!tags.ford && tags.ford !== 'no', closed });
                }
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
    return { lines, lakes };
}

/** Junctions: vertices that two road lines meet at, or that one line passes twice, with the widest half-width there. */
export function junctions(lines: Line[]): Map<string, { x: number; y: number; h: number }> {
    const seen = new Map<string, { x: number; y: number; h: number; count: number }>();
    for (const line of lines) {
        const n = line.z.length - (line.closed ? 1 : 0);
        for (let v = 0; v < n; v++) {
            const x = line.xy[2 * v], y = line.xy[2 * v + 1], key = `${x},${y}`;
            const entry = seen.get(key) ?? { x, y, h: 0, count: 0 };
            entry.count++;
            entry.h = Math.max(entry.h, line.h);
            seen.set(key, entry);
        }
    }
    return new Map([...seen].filter(([, { count }]) => count > 1));
}

/** The unit mitre normal at vertex v of a line: along the line at an end, the bisector of the two segments inside. */
export function mitre(line: Line, v: number): [number, number] {
    const n = line.z.length, dir = (a: number, b: number) => {
        const dx = line.xy[2 * b] - line.xy[2 * a], dy = line.xy[2 * b + 1] - line.xy[2 * a + 1], l = Math.hypot(dx, dy) || 1;
        return [dx / l, dy / l];
    };
    const before = v > 0 ? dir(v - 1, v) : line.closed ? dir(n - 2, n - 1) : undefined;
    const after = v < n - 1 ? dir(v, v + 1) : line.closed ? dir(0, 1) : undefined;
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
export function nodeStamps(list: Feature[], tile: Node, node: Node, baked = false): NodeStamps {
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
        const stamps = shows(line.kind, line.h, gateTexel(node.level), baked);
        if (!stamps && node.level < NEAR) return;
        // A path acts no farther than its bench, and on the near levels than the mask reads it.
        const reach = line.kind === PATH ? line.h + (node.level >= NEAR ? MASK_RANGE_M : RULES.BENCH_REACH) : REACH_M;
        const n = line.z.length, end = (v: number) => !line.closed && (v === 0 || v === n - 1);
        for (let v = 0; v + 1 < n; v++) {
            const [ax, ay, bx, by] = line.xy.slice(2 * v, 2 * v + 4), [mx, my] = mitre(line, v), [nx, ny] = mitre(line, v + 1);
            const dead = (w: number) => end(w) && !joins.has(`${line.xy[2 * w]},${line.xy[2 * w + 1]}`);
            const flags = (line.ford ? FORD : 0) | (dead(v) ? CORE_A : 0) | (dead(v + 1) ? CORE_B : 0) | (end(v) ? CONE_A : 0) | (end(v + 1) ? CONE_B : 0);
            const touched = within([Math.min(ax, bx), Math.min(ay, by), Math.max(ax, bx), Math.max(ay, by)], reach);
            if (!touched.length) continue;
            const at = record([ax, ay, bx, by, line.kind, line.code | (flags << 8), f(line.z[v]), f(line.z[v + 1]), f(line.h), f(mx), f(my), f(nx), f(ny), id]);
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
