// The furniture of the tracks of a node: a mast at each `catenary-mast` point of the bake with two arms
// out over its track, the contact wire over each electrified track as a cable of the anchored mesh, and the platforms: slabs at their
// heights from the bake, whose sides reach down into the drawn surface. Each node draws the part of an
// area platform inside its square.
import earcut from 'earcut';
import { EXTENT, type Feature } from '../stream/features';
import { BRIDGE, metresPerUnit, RAIL, RAIL_CODE, type Line, type Node } from '../terrain/bins';
import { ABSOLUTE, type AnchoredBuilder } from './anchored';
import { GROUND, MeshBuilder, rgb, station } from './mesh';
import type { Owns } from './node';

/** The contact wire's height over the rail. */
const WIRE_M = 5.5;
/**
 * A mast: its side, its top over the rail, the heights of its two arms over the rail, how far they reach
 * past the track's middle and their side; on a deck it hangs this far below the rail, else it stands this
 * deep in the drawn surface.
 */
const MAST = { side: 0.24, top: 7.2, upper: 6.6, lower: WIRE_M, past: 0.4, arm: 0.08, deck: 1.0, sink: 0.5 };
/** A mast's track lies within this many metres of it. */
const MAST_REACH_M = 4;
/** A platform's sides reach this deep into the drawn surface; a platform mapped as a line is this wide without a `width` tag. */
const PLATFORM_SINK_M = 0.5, PLATFORM_WIDTH_M = 3;

const STEEL = rgb('#7a7670'), WIRE = rgb('#3a3c3e'), PLATFORM = rgb('#b4b0a6'), PLATFORM_SIDE = rgb('#8e8b83');

/** A world point with its height in metres. */
type Point = [number, number, number];

/** `square` is the node's square as `owns` reads it: west, north, east and south in world units. */
export function rails(b: MeshBuilder, wires: AnchoredBuilder, list: Feature[], tile: Node, lines: Line[], owns: Owns, square: number[]): void {
    const tracks = lines.filter(line => (line.kind === RAIL || line.kind === BRIDGE) && line.code >= RAIL_CODE);
    for (const line of tracks) {
        if (!line.wire) continue;
        for (let v = 0; v + 1 < line.z.length; v++) {
            if (!owns(line.xy[2 * v], line.xy[2 * v + 1])) continue;
            const span = wires.span([line.xy[2 * v], line.xy[2 * v + 1]], [line.xy[2 * v + 2], line.xy[2 * v + 3]], line.z[v] + WIRE_M, line.z[v + 1] + WIRE_M, 0, ABSOLUTE);
            wires.cable(span, 0, WIRE);
        }
    }
    const scale = 2 ** (17 - tile.level), ox = tile.x * EXTENT, oy = tile.y * EXTENT;
    const world = (xy: Int32Array, v: number): [number, number] => [(ox + xy[2 * v]) * scale, (oy + xy[2 * v + 1]) * scale];
    for (const { kind, tags, parts } of list) {
        if (kind === 'catenary-mast') {
            for (const { xy, z } of parts) {
                const [x, y] = world(xy, 0);
                if (z && !Number.isNaN(z[0]) && owns(x, y)) mast(b, x, y, z[0], tags.on === 'deck', tracks);
            }
        } else if (kind === 'platform' && parts[0]?.z) {
            platform(b, parts.map(({ xy, z }) => clip(Array.from({ length: xy.length / 2 }, (_, v): Point => [...world(xy, v), z![v]]), square)), parts.map(({ outer }) => outer), square);
        } else if (kind === 'platform-line') {
            const width = parseFloat(tags.width ?? '') || PLATFORM_WIDTH_M;
            for (const { xy, z } of parts) {
                if (z) platformLine(b, Array.from({ length: xy.length / 2 }, (_, v) => world(xy, v)).flat(), Array.from(z), width / 2, owns);
            }
        }
    }
}

/** A mast at world point x, y beside the nearest track, `z` its rail's height; on a deck it stands against the deck's side, below its rail. */
function mast(b: MeshBuilder, x: number, y: number, z: number, deck: boolean, tracks: Line[]): void {
    const unit = metresPerUnit(y);
    let best: { fx: number; fy: number; d: number } | undefined;
    for (const line of tracks) for (let v = 0; v + 1 < line.z.length; v++) {
        const [ax, ay, bx, by] = line.xy.slice(2 * v, 2 * v + 4), dx = bx - ax, dy = by - ay;
        const u = Math.min(1, Math.max(0, ((x - ax) * dx + (y - ay) * dy) / (dx * dx + dy * dy || 1)));
        const fx = ax + u * dx, fy = ay + u * dy, d = Math.hypot(x - fx, y - fy) * unit;
        if (d < MAST_REACH_M && (!best || d < best.d)) best = { fx, fy, d };
    }
    if (!best) return;
    const s = MAST.side / 2, top = z + MAST.top, colour = STEEL;
    const bottom = deck ? z - MAST.deck : -MAST.sink, base = colour | (deck ? 0 : GROUND << 24);
    // The corners east-south, east-north, west-north and west-south, at the foot and at the top; the faces share them.
    const square = [[s, -s], [s, s], [-s, s], [-s, -s]], out: [number, number, number][] = [[1, 0, 0], [0, 1, 0], [-1, 0, 0], [0, -1, 0]];
    const foot = square.map(([e, n]) => b.vertex(x + e / unit, y - n / unit, bottom, base)), head = square.map(([e, n]) => b.vertex(x + e / unit, y - n / unit, top, colour));
    for (let k = 0; k < 4; k++) b.quad([foot[k], foot[(k + 1) % 4], head[(k + 1) % 4], head[k]], out[k]);
    b.quad([head[0], head[1], head[2], head[3]], [0, 0, 1]);
    // The arms run over the track's middle and a little past it.
    const reach = (best.d + MAST.past) / best.d, ex = x + (best.fx - x) * reach, ey = y + (best.fy - y) * reach;
    for (const height of [MAST.upper, MAST.lower]) bar(b, [x, y, z + height], [ex, ey, z + height], MAST.arm / 2, colour);
}

/** A bar `half` metres to each side of the way from p to q, which is not vertical: its faces up, down and to its sides. */
function bar(b: MeshBuilder, p: Point, q: Point, half: number, colour: number): void {
    const unit = metresPerUnit(p[1]), east = (q[0] - p[0]) * unit, north = -(q[1] - p[1]) * unit, l = Math.hypot(east, north) || 1;
    // Level and across the bar, east and north.
    const side = [-north / l, east / l];
    // The corners at each end, around the bar from its top on the side away from `side`; the faces share them.
    const ring = ([x, y, z]: Point) => [[-1, 1], [1, 1], [1, -1], [-1, -1]].map(([a, c]) => b.vertex(x + side[0] * a * half / unit, y - side[1] * a * half / unit, z + c * half, colour));
    const [from, to] = [ring(p), ring(q)], out: [number, number, number][] = [[0, 0, 1], [side[0], side[1], 0], [0, 0, -1], [-side[0], -side[1], 0]];
    for (let k = 0; k < 4; k++) b.quad([from[k], from[(k + 1) % 4], to[(k + 1) % 4], to[k]], out[k]);
}

/** A ring of world points with heights cut to `square` (Sutherland–Hodgman); heights run straight along a cut edge. */
function clip(ring: Point[], [x0, y0, x1, y1]: number[]): Point[] {
    for (const [axis, bound, keep] of [[0, x0, 1], [0, x1, -1], [1, y0, 1], [1, y1, -1]] as const) {
        const input = ring;
        ring = [];
        input.forEach((p, i) => {
            const q = input[(i + 1) % input.length], inside = (p[axis] - bound) * keep >= 0;
            if (inside) ring.push(p);
            if (inside !== (q[axis] - bound) * keep >= 0) {
                const t = (bound - p[axis]) / (q[axis] - p[axis]), cut = p.map((c, k) => c + t * (q[k] - c)) as Point;
                cut[axis] = bound;
                ring.push(cut);
            }
        });
    }
    return ring;
}

/** A platform from its rings cut to `square`: its top, and its sides down into the drawn surface, but none along the square's edges. */
function platform(b: MeshBuilder, rings: Point[][], outer: boolean[], square: number[]): void {
    const cut = (p: Point, q: Point) => [0, 1].some(a => [square[a], square[a + 2]].some(s => p[a] === s && q[a] === s));
    for (let r = 0; r < rings.length; r++) {
        if (!outer[r] || rings[r].length < 3) continue;
        const [x0, y0] = rings[r][0], unit = metresPerUnit(y0), polygon = [r], holes: number[] = [];
        for (let k = r + 1; k < rings.length && !outer[k]; k++) if (rings[k].length >= 3) polygon.push(k);
        const flat: number[] = [], first: number[] = [];
        for (const k of polygon) {
            if (k !== r) holes.push(flat.length / 2);
            first.push(flat.length / 2);
            for (const [x, y] of rings[k]) flat.push((x - x0) * unit, (y0 - y) * unit);
        }
        const top = polygon.flatMap(k => rings[k].map(([x, y, z]) => b.vertex(x, y, z, PLATFORM)));
        const triangles = earcut(flat, holes);
        for (let i = 0; i < triangles.length; i += 3) b.triangle(top[triangles[i]], top[triangles[i + 1]], top[triangles[i + 2]], [0, 0, 1]);
        polygon.forEach((k, j) => {
            const ring = rings[k], n = ring.length, at = first[j];
            // Positive for a ring that runs counter-clockwise, east and north: its outside lies to the right of each edge.
            let area = 0;
            for (let v = 0; v < n; v++) area += flat[2 * (at + v)] * flat[2 * (at + (v + 1) % n) + 1] - flat[2 * (at + (v + 1) % n)] * flat[2 * (at + v) + 1];
            const sign = (area > 0 ? 1 : -1) * (k === r ? 1 : -1);
            for (let v = 0; v < n; v++) {
                const w = (v + 1) % n, east = flat[2 * (at + w)] - flat[2 * (at + v)], north = flat[2 * (at + w) + 1] - flat[2 * (at + v) + 1];
                if (cut(ring[v], ring[w])) continue;
                const [[ax, ay, az], [bx, by, bz]] = [ring[v], ring[w]];
                b.quad([b.vertex(ax, ay, az, PLATFORM_SIDE), b.vertex(bx, by, bz, PLATFORM_SIDE), b.vertex(bx, by, -PLATFORM_SINK_M, PLATFORM_SIDE | (GROUND << 24)), b.vertex(ax, ay, -PLATFORM_SINK_M, PLATFORM_SIDE | (GROUND << 24))], [sign * north, -sign * east, 0]);
            }
        });
    }
}

/** A platform along a line of world points with a height at each vertex, `half` metres to each side: the segments that a square `owns`. */
function platformLine(b: MeshBuilder, xy: number[], z: number[], half: number, owns: Owns): void {
    const line: Line = { kind: 0, xy, z, h: half, code: 0, ford: false, profile: 0 }, n = z.length;
    const ground = PLATFORM_SIDE | (GROUND << 24);
    for (let v = 0; v + 1 < n; v++) {
        if (!owns(xy[2 * v], xy[2 * v + 1])) continue;
        const s = station(line, v), t = station(line, v + 1);
        b.strip(s, t, [-half, 0], [half, 0], PLATFORM, [0, 1]);
        for (const side of [1, -1]) {
            b.quad([b.at(s, [side * half, 0], PLATFORM_SIDE), b.at(t, [side * half, 0], PLATFORM_SIDE), b.vertex(t.x + t.ax * side * half, t.y + t.ay * side * half, -PLATFORM_SINK_M, ground), b.vertex(s.x + s.ax * side * half, s.y + s.ay * side * half, -PLATFORM_SINK_M, ground)], b.outward(s, t, [side, 0]));
        }
        for (const [end, st, sign] of [[0, s, -1], [n - 1, t, 1]] as [number, typeof s, number][]) {
            if (v !== (end ? n - 2 : 0)) continue;
            const dx = xy[2 * v + 2] - xy[2 * v], dy = xy[2 * v + 3] - xy[2 * v + 1], l = Math.hypot(dx, dy) || 1;
            b.quad([b.at(st, [-half, 0], PLATFORM_SIDE), b.at(st, [half, 0], PLATFORM_SIDE), b.vertex(st.x + st.ax * half, st.y + st.ay * half, -PLATFORM_SINK_M, ground), b.vertex(st.x - st.ax * half, st.y - st.ay * half, -PLATFORM_SINK_M, ground)], [sign * dx / l, -sign * dy / l, 0]);
        }
    }
}
