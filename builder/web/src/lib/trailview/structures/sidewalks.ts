// The sidewalks of the town roads as slabs that draw near the eye: paving RISE_M over the road at its
// solved height and cross-slope, a granite kerb with its face toward the road, and a skirt down into the
// drawn ground on the outer side. The stamps hold the ground under a slab at the road's height. A slab
// stops where a carriageway crosses it, at a side road's mouth; where two roads' sidewalks meet in a
// corner, the road of the first class in roads.json, then the first line, keeps it. A road that OSM
// splits runs on into its next line across their shared mitre. Farther off, the mask paints them.
import { crossSlope, metresPerUnit, PAVING_CODE, PROFILES, RAIL, ROAD, type Line } from '../terrain/bins';
import { gridOf, LEFT, near, RIGHT, type Grid } from '../terrain/town';
import { GROUND, mix, PAINT, rgb, station, type MeshBuilder, type Station } from './mesh';
import type { Owns } from './node';

/** The slab's height over the road and the width of its granite kerb, as KERB_M of roads.wgsl; its kerb's face reaches FOOT_M under the road, its skirt SINK_M into the drawn ground. */
const RISE_M = 0.12, KERB_M = 0.18, FOOT_M = 0.05, SINK_M = 0.3;
/** A slab is tested in pieces about PIECE_M long; BISECTIONS halvings place the ends of its runs. */
const PIECE_M = 0.5, BISECTIONS = 4;
/** No sidewalk reaches farther from its road's line. */
const REACH_M = 10;
/** Two ends of roads that meet at most this straight run on into each other: the cosine of the angle between their ways out. */
const RUNS_ON = 0.7;
const PAVED = (rgb('#000000', PAINT) | PAVING_CODE) >>> 0, GRANITE = rgb('#c3c0b8'), EDGING = rgb('#a8a59e');

/** The carriageways of a tile in a grid of their segments, and per road end the end of the road it runs on into. */
interface Roads { grid: Grid; segments: { line: Line; v: number; rank: number }[]; partner: Map<Line, [Line?, Line?]> }
const known = new WeakMap<Line[], Roads>();

function roadsOf(lines: Line[]): Roads {
    const found = known.get(lines);
    if (found) return found;
    const segments: Roads['segments'] = [], boxes: number[][] = [], key = (line: Line, v: number) => `${line.xy[2 * v]},${line.xy[2 * v + 1]}`;
    const ends = new Map<string, { line: Line; end: 0 | 1 }[]>(), inner = new Set<string>();
    lines.forEach((line, rank) => {
        if (line.kind !== ROAD && line.kind !== RAIL) return;
        const n = line.z.length;
        for (let v = 0; v + 1 < n; v++) {
            const [ax, ay, bx, by] = line.xy.slice(2 * v, 2 * v + 4);
            segments.push({ line, v, rank });
            boxes.push([Math.min(ax, bx), Math.min(ay, by), Math.max(ax, bx), Math.max(ay, by)]);
            if (v > 0) inner.add(key(line, v));
        }
        if (line.kind === ROAD) for (const end of [0, 1] as const) ends.set(key(line, end ? n - 1 : 0), [...(ends.get(key(line, end ? n - 1 : 0)) ?? []), { line, end }]);
    });
    // Where no road passes through, the two ends that run most nearly straight on pair up.
    const partner = new Map<Line, [Line?, Line?]>();
    for (const [at, here] of ends) {
        if (inner.has(at) || here.length < 2) continue;
        const away = ({ line, end }: (typeof here)[number]) => {
            const n = line.z.length, [v, w] = end ? [n - 1, n - 2] : [0, 1], dx = line.xy[2 * w] - line.xy[2 * v], dy = line.xy[2 * w + 1] - line.xy[2 * v + 1], l = Math.hypot(dx, dy) || 1;
            return [dx / l, dy / l];
        };
        let best: [number, number] | undefined, least = RUNS_ON;
        for (let i = 0; i < here.length; i++) for (let k = i + 1; k < here.length; k++) {
            const [p, q] = [away(here[i]), away(here[k])], cos = p[0] * q[0] + p[1] * q[1];
            if (cos < least) [least, best] = [cos, [i, k]];
        }
        if (!best) continue;
        for (const [a, b] of [[best[0], best[1]], [best[1], best[0]]]) (partner.get(here[a].line) ?? partner.set(here[a].line, [undefined, undefined]).get(here[a].line)!)[here[a].end] = here[b].line;
    }
    const roads = { grid: gridOf(boxes), segments, partner };
    known.set(lines, roads);
    return roads;
}

/** The width of a line's sidewalks. */
const walkOf = (line: Line) => Object.values(PROFILES)[line.profile].walk;

/**
 * The station of a line's vertex v across the bisector of the segments about it, where the carriageway's
 * edges meet, also at an OSM vertex of a rounded bend; at an end that runs on into a partner, across the
 * bisector of the two lines, as both take it.
 */
function stationAt(line: Line, v: number, partner?: Line): Station {
    const n = line.z.length;
    if (!partner || (v !== 0 && v !== n - 1)) return station({ ...line, anchors: undefined }, v);
    const pn = partner.z.length, x = line.xy[2 * v], y = line.xy[2 * v + 1];
    const w = partner.xy[0] === x && partner.xy[1] === y ? 1 : pn - 2, other = [partner.xy[2 * w], partner.xy[2 * w + 1]];
    const near = v ? [line.xy[2 * v - 2], line.xy[2 * v - 1]] : [line.xy[2], line.xy[3]];
    const xy = v ? [...near, x, y, ...other] : [...other, x, y, ...near];
    return { ...station({ ...line, xy, z: [0, line.z[v], 0], anchors: undefined }, 1), z: line.z[v] };
}

/** The slabs of the sidewalks of the lines whose segments' first vertex a node `owns`. */
export function sidewalks(b: MeshBuilder, lines: Line[], owns: Owns): void {
    const { grid, segments, partner } = roadsOf(lines);
    lines.forEach((line, rank) => {
        if (!line.sidewalks) return;
        const n = line.z.length, h = line.h, walk = walkOf(line), ends = partner.get(line), unit = metresPerUnit(line.xy[1]);
        const stations = new Map<number, Station>();
        const at = (v: number) => stations.get(v) ?? stations.set(v, stationAt(line, v, v === 0 ? ends?.[0] : v === n - 1 ? ends?.[1] : undefined)).get(v)!;
        // The segments of the carriageways near the slab of segment v on a side, once each.
        const nearby = new Map<number, Roads['segments']>();
        const around = (v: number, side: number) => {
            let found = nearby.get(2 * v + side);
            if (!found) {
                const reach = (h + walk + REACH_M) / unit, [ax, ay, bx, by] = line.xy.slice(2 * v, 2 * v + 4), list: Roads['segments'] = [];
                near(grid, ax, ay, bx, by, reach, k => list.push(segments[k]));
                nearby.set(2 * v + side, (found = list));
            }
            return found;
        };
        /** Whether the slab of segment v on a side is open a share u along it: no carriageway there, and no sidewalk of a road that keeps a corner. */
        const open = (v: number, side: number, u: number) => {
            const p = mix(at(v), at(v + 1), u), middle = (side === LEFT ? 1 : -1) * (h + walk / 2), x = p.x + p.ax * middle, y = p.y + p.ay * middle;
            return around(v, side).every(({ line: other, v: w, rank: order }) => {
                const ax = other.xy[2 * w], ay = other.xy[2 * w + 1], dx = other.xy[2 * w + 2] - ax, dy = other.xy[2 * w + 3] - ay, l2 = dx * dx + dy * dy || 1;
                const along = ((x - ax) * dx + (y - ay) * dy) / l2, cross = (dx * (y - ay) - dy * (x - ax)) / Math.sqrt(l2) * unit;
                const t0 = Math.min(1, Math.max(0, along)), d = Math.sqrt((x - ax - t0 * dx) ** 2 + (y - ay - t0 * dy) ** 2) * unit;
                if (d < other.h) return false;
                return other === line || !other.sidewalks || along < 0 || along > 1 || Math.abs(cross) > other.h + walkOf(other) || !(other.sidewalks[w] & (cross > 0 ? RIGHT : LEFT))
                    || !!ends?.includes(other) || other.profile > line.profile || (other.profile === line.profile && order > rank);
            });
        };
        // Whether the slab of segment v on a side runs on into the next segment's (step 1) or the one before's (step -1); not into another line.
        const runsOn = (v: number, side: number, step: number) => v + step >= 0 && v + step + 1 < n && !!(line.sidewalks![v + step] & side) && open(v + step, side, step > 0 ? 0 : 1);
        for (let v = 0; v + 1 < n; v++) {
            if (!line.sidewalks[v] || !owns(line.xy[2 * v], line.xy[2 * v + 1])) continue;
            const s = at(v), t = at(v + 1), length = Math.hypot(t.x - s.x, t.y - s.y) * unit;
            // At an end that runs on into a partner of another width, the kerb and the outer edge meet the partner's halfway.
            const meet = (w: number, end: 0 | 1, own: number, of: (other: Line) => number) => (w === (end ? n - 1 : 0) && ends?.[end] ? (own + of(ends[end]!)) / 2 : own);
            const halves: [number, number] = [meet(v, 0, h, other => other.h), meet(v + 1, 1, h, other => other.h)];
            const walks: [number, number] = [meet(v, 0, walk, walkOf), meet(v + 1, 1, walk, walkOf)];
            const pieces = Math.max(1, Math.ceil(length / PIECE_M)), centre = (k: number) => (k + 0.5) / pieces;
            for (const side of [LEFT, RIGHT]) {
                if (!(line.sidewalks[v] & side)) continue;
                // A run of open pieces ends between the centre of its last piece and of the next, where the slab stops.
                const edge = (inside: number, outside: number) => {
                    for (let i = 0; i < BISECTIONS; i++) {
                        const half = (inside + outside) / 2;
                        if (open(v, side, half)) inside = half;
                        else outside = half;
                    }
                    return inside;
                };
                const kept = Array.from({ length: pieces }, (_, k) => open(v, side, centre(k)));
                for (let k = 0; k < pieces; k++) {
                    if (!kept[k] || (k > 0 && kept[k - 1])) continue;
                    let last = k;
                    while (last + 1 < pieces && kept[last + 1]) last++;
                    const u0 = k > 0 ? edge(centre(k), centre(k - 1)) : 0, u1 = last + 1 < pieces ? edge(centre(last), centre(last + 1)) : 1;
                    slab(b, line, v, s, t, length, u0, u1, side === LEFT ? 1 : -1, halves, walks, [u0 > 0 || !runsOn(v, side, -1), u1 < 1 || !runsOn(v, side, 1)]);
                }
            }
        }
    });
}

/**
 * The slab from share u0 to u1 of segment v from station s to t, on the side `sign`, its kerb `halves` and its
 * width `walks` metres at s and at t, with a face at its start and at its end where `faces` has them.
 */
function slab(b: MeshBuilder, line: Line, v: number, s: Station, t: Station, length: number, u0: number, u1: number, sign: number, halves: [number, number], walks: [number, number], faces: [boolean, boolean]): void {
    const across = (u: number, from: number) => sign * (halves[0] + (halves[1] - halves[0]) * u + from * (walks[0] + (walks[1] - walks[0]) * u));
    const inner = (u: number) => across(u, 0), outer = (u: number) => across(u, 1), kerb = (u: number) => inner(u) + sign * KERB_M;
    const at = (u: number) => mix(s, t, u);
    // A corner `a` metres to the left, `up` metres over the road there, with its cross-slope; on the drawn ground with GROUND.
    const corner = (u: number, a: number, up: number, colour: number) => {
        const p = at(u), x = p.x + p.ax * a, y = p.y + p.ay * a;
        return colour >>> 24 & GROUND ? b.vertex(x, y, up, colour) : b.vertex(x, y, p.z - a * crossSlope(line, v, u * length, (1 - u) * length) + up, colour);
    };
    const out = b.outward(at(u0), at(u1), [sign, 0]), up: [number, number, number] = [0, 0, 1];
    const quad = (a: (u: number) => number, c: (u: number) => number, lo: number, hi: number, colour: number, facing: [number, number, number]) =>
        b.quad([corner(u0, a(u0), lo, colour), corner(u1, a(u1), lo, colour), corner(u1, c(u1), hi, colour), corner(u0, c(u0), hi, colour)], facing);
    quad(kerb, outer, RISE_M, RISE_M, PAVED, up);
    quad(inner, kerb, RISE_M, RISE_M, GRANITE, up);
    quad(inner, inner, -FOOT_M, RISE_M, GRANITE, [-out[0], -out[1], 0]);
    b.quad([corner(u0, outer(u0), RISE_M, EDGING), corner(u1, outer(u1), RISE_M, EDGING), corner(u1, outer(u1), -SINK_M, EDGING | (GROUND << 24)), corner(u0, outer(u0), -SINK_M, EDGING | (GROUND << 24))], out);
    const dx = t.x - s.x, dy = t.y - s.y, l = Math.hypot(dx, dy) || 1;
    faces.forEach((face, end) => {
        const u = end ? u1 : u0, facing: [number, number, number] = [(end ? 1 : -1) * dx / l, -(end ? 1 : -1) * dy / l, 0];
        if (face) b.quad([corner(u, inner(u), -FOOT_M, GRANITE), corner(u, outer(u), -FOOT_M, GRANITE), corner(u, outer(u), RISE_M, GRANITE), corner(u, inner(u), RISE_M, GRANITE)], facing);
    });
}
