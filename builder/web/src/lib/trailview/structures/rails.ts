// The furniture of the tracks of a node: the contact line over each electrified track, with a mast at each
// `catenary-mast` point of the bake, and the platforms: slabs at their heights from the bake, whose sides reach down
// into the drawn surface. Each node draws the part of an area platform inside its square.
import earcut from 'earcut';
import { MESSENGER_M, railMast, WIRE_M } from '../models/rail-mast';
import { EXTENT, unitScale, type Feature } from '../stream/features';
import { BRIDGE, metresPerUnit, RAIL, RAIL_CODE, type Line, type Node } from '../terrain/bins';
import { ABSOLUTE, type AnchoredBuilder } from './anchored';
import { stationsOf } from '../roads/stations';
import { GROUND, MeshBuilder, rgb, station } from './mesh';
import type { Owns } from './node';

/**
 * The contact line: a contact wire WIRE_M over the rail and a messenger wire over it, which lies in each mast's saddle
 * and hangs SAG_M lower from HALF_SPAN_M beside a mast on; a dropper joins the two each DROPPER_M from a mast. Both
 * wires follow the curve that draws the track, straight where that leaves a straight way by less than FIT_M. Past the
 * near field a mast is thinner than a pixel: only the contact wire draws at any distance.
 *
 * A wire bends at fixed places only: at each vertex of the tile, each PIECE_M from it where no mast is within
 * HALF_SPAN_M (there the droppers divide it), at each mast and each dropper.
 * So the squares and the tiles that share a track cut its wires into the same pieces, and each draws the pieces that
 * start in it. A piece reaches PIECE_M past a square at most, and a mast shapes the wire HALF_SPAN_M about it: the masts
 * within REACH_M of a square are all that its wires need, and a tile's buffer of 40 m holds them.
 */
const SAG_M = 0.95, HALF_SPAN_M = 25, DROPPER_M = HALF_SPAN_M / 3, FIT_M = 0.05, PIECE_M = 10;
/** The wires' diameters: thicker than a real wire, so a wire near the eye is a line and not a band. */
const CONTACT_M = 0.03, MESSENGER_WIRE_M = 0.025, DROPPER_WIRE_M = 0.012;
/** A mast's track lies within this many metres of it. */
const MAST_REACH_M = 4, REACH_M = HALF_SPAN_M + PIECE_M + MAST_REACH_M;
/** A platform's sides reach this deep into the drawn surface; a platform mapped as a line is this wide without a `width` tag. */
const PLATFORM_SINK_M = 0.5, PLATFORM_WIDTH_M = 3;

const WIRE = rgb('#3a3c3e'), PLATFORM = rgb('#b4b0a6'), PLATFORM_SIDE = rgb('#8e8b83');

/** A world point with its height in metres. */
type Point = [number, number, number];
/** A place on a track's curve: a world point, the rail's height, the metres along the curve, and whether a wire bends there whatever the curve does. */
interface Stop { x: number; y: number; z: number; along: number; fixed?: boolean; piece?: boolean }

const masts = new Map<string, ReturnType<typeof railMast>>();
/** The stops of each track's curve, made once, and their box: west, north, east and south. */
const curves = new WeakMap<Line, { stops: Stop[]; box: number[] }>();

function curveOf(line: Line): { stops: Stop[]; box: number[] } {
    let curve = curves.get(line);
    if (!curve) {
        // A wire bends at each vertex of the tile, which is the last stop of its span, and each PIECE_M from it.
        const stops: Stop[] = [];
        let from = 0;
        stationsOf(line, () => line.h).forEach(({ x, y, z, along, v }, i, all) => {
            const vertex = !i || i === all.length - 1 || all[i + 1].v !== v, before = stops[stops.length - 1];
            if (before) for (let at = from + PIECE_M * (Math.floor((before.along - from) / PIECE_M) + 1); at < along - 1e-6; at += PIECE_M) stops.push({ ...between(before, { x, y, z, along }, (at - before.along) / (along - before.along)), fixed: true, piece: true });
            stops.push({ x, y, z, along, fixed: vertex });
            if (vertex) from = along;
        });
        const xs = stops.map(s => s.x), ys = stops.map(s => s.y);
        curves.set(line, (curve = { stops, box: [Math.min(...xs), Math.min(...ys), Math.max(...xs), Math.max(...ys)] }));
    }
    return curve;
}

/**
 * `far` draws at any distance, `near` only near the eye; `square` is the node's square as `owns` reads it: west,
 * north, east and south in world units.
 */
export function rails(b: MeshBuilder, far: AnchoredBuilder, near: AnchoredBuilder, list: Feature[], tile: Node, lines: Line[], owns: Owns, square: number[]): void {
    const tracks = lines.filter(line => (line.kind === RAIL || line.kind === BRIDGE) && line.code >= RAIL_CODE);
    const scale = unitScale(tile.level), ox = tile.x * EXTENT, oy = tile.y * EXTENT;
    const world = (xy: ArrayLike<number>, v: number): [number, number] => [(ox + xy[2 * v]) * scale, (oy + xy[2 * v + 1]) * scale];
    // The masts and the tracks that reach this node's wires.
    const reach = REACH_M / metresPerUnit(square[1]);
    const within = (x: number, y: number) => x >= square[0] - reach && x <= square[2] + reach && y >= square[1] - reach && y <= square[3] + reach;
    const wired = tracks.filter(line => line.wire && line.z.some((_, v) => within(line.xy[2 * v], line.xy[2 * v + 1]) || (v > 0 && crosses(line.xy, v - 1, square, reach)))).map(line => ({ ...curveOf(line), stands: [] as number[] }));
    for (const { kind, tags, parts } of list) {
        if (kind === 'catenary-mast') {
            for (const { xy, z } of parts) {
                const [x, y] = world(xy, 0);
                if (!z || Number.isNaN(z[0]) || !within(x, y)) continue;
                // The mast stands at the nearest track: its cantilever ends over that place of the curve.
                let best: { stop: Stop; d: number; stands: number[] } | undefined;
                const close = MAST_REACH_M / metresPerUnit(y);
                for (const { stops, box, stands } of wired) {
                    if (x < box[0] - close || x > box[2] + close || y < box[1] - close || y > box[3] + close) continue;
                    const [stop, d] = nearest(stops, x, y);
                    if (d < MAST_REACH_M && (!best || d < best.d)) best = { stop, d, stands };
                }
                if (!best) continue;
                best.stands.push(best.stop.along);
                if (!owns(x, y)) continue;
                const span = Math.round(best.d * 10) / 10, name = `${span} ${tags.on}`, model = masts.get(name) ?? masts.set(name, railMast(span, tags.on === 'deck')).get(name)!;
                const anchor = far.span([x, y], [best.stop.x, best.stop.y], best.stop.z, best.stop.z, 0, ABSOLUTE);
                near.model(model, anchor, 1);
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
    for (const { stops, stands } of wired) {
        // Only the stretch of the curve that reaches this node, from a vertex of the tile to a vertex of the tile.
        let from = stops.findIndex(s => within(s.x, s.y)), to = stops.length - 1;
        if (from < 0) continue;
        while (!within(stops[to].x, stops[to].y)) to--;
        while (!stops[from].fixed || stops[from].piece) from--;
        while (!stops[to].fixed || stops[to].piece) to++;
        wires(far, near, stops.slice(from, to + 1), stands, owns);
    }
}

/** Whether the segment from vertex v of a line passes within `reach` of a square, by its box. */
function crosses(xy: number[], v: number, [x0, y0, x1, y1]: number[], reach: number): boolean {
    const [ax, ay, bx, by] = xy.slice(2 * v, 2 * v + 4);
    return Math.max(ax, bx) >= x0 - reach && Math.min(ax, bx) <= x1 + reach && Math.max(ay, by) >= y0 - reach && Math.min(ay, by) <= y1 + reach;
}

/** The place between stops a and b a share u of the way. */
const between = (a: Omit<Stop, 'fixed'>, b: Omit<Stop, 'fixed'>, u: number): Stop => ({ x: a.x + (b.x - a.x) * u, y: a.y + (b.y - a.y) * u, z: a.z + (b.z - a.z) * u, along: a.along + (b.along - a.along) * u });

/** The place of a curve nearest to world point x, y, and the metres to it. */
function nearest(stops: Stop[], x: number, y: number): [Stop, number] {
    let best = stops[0], least = Infinity;
    for (let i = 0; i + 1 < stops.length; i++) {
        const a = stops[i], c = stops[i + 1], dx = c.x - a.x, dy = c.y - a.y;
        const at = between(a, c, Math.min(1, Math.max(0, ((x - a.x) * dx + (y - a.y) * dy) / (dx * dx + dy * dy || 1)))), d = Math.hypot(x - at.x, y - at.y);
        if (d < least) [best, least] = [at, d];
    }
    return [best, least * metresPerUnit(y)];
}

/** The two wires of a track from the stops of its curve, and their droppers; `stands` are the metres along it of its masts. */
function wires(far: AnchoredBuilder, near: AnchoredBuilder, stops: Stop[], stands: number[], owns: Owns): void {
    const [start, total] = [stops[0].along, stops[stops.length - 1].along], toMast = (along: number) => stands.reduce((least, s) => Math.min(least, Math.abs(s - along)), Infinity);
    // The messenger wire over the contact wire: a parabola from each mast's saddle down to its lowest, HALF_SPAN_M on.
    const rise = (along: number) => MESSENGER_M - WIRE_M - SAG_M * (1 - (1 - Math.min(1, toMast(along) / HALF_SPAN_M)) ** 2);
    // A dropper hangs each DROPPER_M from a mast, up to the middle between two masts.
    const droppers = stands.flatMap(s => [-3, -2, -1, 1, 2, 3].map(k => s + k * DROPPER_M)).filter(d => d > start && d < total && toMast(d) > Math.round(toMast(d) / DROPPER_M) * DROPPER_M - 1e-6).sort((a, c) => a - c).filter((d, i, all) => !i || d - all[i - 1] > 1);
    // The wires bend at each fixed stop, each mast and each dropper, and between them where the curve asks.
    const fixed = [...new Set([...stands, ...droppers])].sort((a, c) => a - c), hung = new Set(droppers), path: (Stop & { hung?: boolean })[] = [];
    let next = fixed.findIndex(at => at > start);
    if (next < 0) next = fixed.length;
    for (let i = 0; i < stops.length; i++) {
        // Near a mast the droppers divide the wire: the places each PIECE_M are not needed there.
        if (!stops[i].piece || toMast(stops[i].along) >= HALF_SPAN_M || fixed[next] === stops[i].along) path.push({ ...stops[i] });
        for (; i + 1 < stops.length && next < fixed.length && fixed[next] < stops[i + 1].along; next++) {
            if (fixed[next] > stops[i].along) path.push({ ...between(stops[i], stops[i + 1], (fixed[next] - stops[i].along) / (stops[i + 1].along - stops[i].along)), fixed: true });
            else path[path.length - 1].fixed = true;
            path[path.length - 1].hung ||= hung.has(fixed[next]);
        }
    }
    const unit = metresPerUnit(stops[0].y);
    for (let from = 0; from + 1 < path.length;) {
        // The farthest stop that a straight wire reaches within FIT_M of the curve.
        let to = from + 1;
        for (; to + 1 < path.length && !path[to].fixed; to++) {
            const a = path[from], c = path[to + 1], fits = path.slice(from + 1, to + 1).every(p => {
                const at = between(a, c, (p.along - a.along) / (c.along - a.along));
                return Math.hypot(p.x - at.x, p.y - at.y) * unit < FIT_M && Math.abs(p.z - at.z) < FIT_M;
            });
            if (!fits) break;
        }
        const a = path[from], c = path[to];
        if (owns(a.x, a.y) && !Number.isNaN(a.z + c.z)) {
            const span = far.span([a.x, a.y], [c.x, c.y], a.z + WIRE_M, c.z + WIRE_M, 0, ABSOLUTE);
            far.cable(span, 0, WIRE, CONTACT_M);
            near.cable(span, 0, WIRE, MESSENGER_WIRE_M, [rise(a.along), rise(c.along)]);
            if (a.hung) near.upright(span, 0, 0, rise(a.along), WIRE, DROPPER_WIRE_M);
        }
        from = to;
    }
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
