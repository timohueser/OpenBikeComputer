// PROTOTYPE road system (a study; never merges). Junctions and crossings of the corridors.
//
// At a vertex that roads share, the through pair (the best class, then the straightest pair) runs on; each
// other road is a side arm. Between two neighbouring ways a corner turns the kerb (or the shoulder) from
// the edge of the one to the edge of the other round an arc: an asphalt fillet fills it, the kerb, the
// sidewalk and the band wrap round it. Each way's side toward a corner stops at the arc's end; a side arm's
// carriageway starts on the through road's edge. A path that meets a road stops at the road's outer edge;
// one that crosses a paved town road gets a zebra across it and lowered kerbs at both ends. A minor road
// that meets a main road gets a give-way line across its mouth. A road bridge's end is a way of the junction
// there that its deck draws (structures/bridges.ts): its corners stay sharp, and a side arm's mouth runs from
// the deck's edge.
//
// The plan is made once per tile's lines: each line's stations with cuts (the faces that the sweep leaves
// out between two stations) and the junction faces that replace them.
import { BRIDGE, isPath, metresPerUnit, PATH, PAVED_CODES, PAVING_CODE, RAIL_CODE, ROAD, type Line } from '../terrain/bins';
import { cornersOf, halfAt, partnersOf, type Corner as StampCorner } from '../terrain/corners';
import { gridOf, near } from '../terrain/town';
import { deckOuter } from '../structures/bridges';
import type { MeshBuilder } from '../structures/mesh';
import type { Owns } from '../structures/node';
import { classLift, corridorColour, quad, vertex, type Ctx, type V } from './corridor';
import { carriageway, CLASS_NAMES, corridorLine, KERB_RISE_M, LIFT_M, MAT, sideOf, type Part } from './section';
import { stationsOf, type Station } from './stations';

/** Materials of the junctions, as MAT_ZEBRA and the others of junctions.wgsl. */
export const JMAT = { ZEBRA: 60, GIVE_WAY: 61, FILLET: 62 } as const;
/** What the sweep leaves out between a station and the next: the carriageway, the left side, the right side. */
export const CUT = { ROAD: 1, LEFT: 2, RIGHT: 4, ALL: 7 } as const;

/** A station of a planned line, with the cuts to the next station. */
export interface JStation extends Station { cut?: number }

/** Corner radii: a town street's kerb, a rural road's edge, a track's; the least radius that rounds a corner, else it stays sharp. */
const TOWN_R = 5, RURAL_R = 7, TRACK_R = 4, LEAST_R = 0.8;
/** The widest turn between two ways that makes a corner where neither is a side arm, and where one is. */
const MAX_TURN = (150 * Math.PI) / 180, MAX_SIDE_TURN = (172 * Math.PI) / 180;
/** The straightest pair of ways at a junction runs on as the through road if its turn is at least this (cosine of the angle between the ways out). */
const THROUGH_COS = -0.3;
/** Columns of a corner's arc. */
const ARC_STEPS = 8;
/** How far a way may leave the straight line from its junction where a corner's arc ends on it. */
const STRAIGHT_M = 0.6;
/** A zebra's half-width along the road, the lowered kerb's ramp past it, and the drop of a lowered kerb: it stays this far over the carriageway. */
const ZEBRA_HALF_M = 2, RAMP_M = 1.2, LOWERED_M = 0.025;
/** The give-way line: this far back from the through road's edge, this wide. */
const GIVE_WAY_BACK_M = 0.4, GIVE_WAY_M = 0.3;
/** The give-way faces reach this far back from the line: the line, then the triangle in the approaching lane (as GIVE_WAY_REACH_M of junctions.wgsl). */
const GIVE_WAY_REACH_M = 4.2;
/** The classes that a minor road gives way to with a line, and the paths that cross a town road on a zebra. */
const MAIN = ['motorway', 'motorway_link', 'trunk', 'trunk_link', 'primary', 'primary_link', 'secondary', 'secondary_link', 'tertiary', 'tertiary_link'];
const ZEBRA_PATHS = ['footway', 'path', 'cycleway', 'pedestrian', 'bridleway'];

/** A way out of a junction: its line, vertex and step, its unit direction (metres east, south), its angle, and the metres it may give to a corner. */
interface Way { line: Line; v: number; step: 1 | -1; u: [number, number]; angle: number; room: number; through?: boolean; side?: boolean }
/** A corner between ways p and q: the arc's centre, radius and ends (metres about the junction), the edges' meeting point, the normals toward it; each way's side of its line toward it. */
interface Corner { p: Way; q: Way; o: number[]; r: number; a: number[]; b: number[]; P: number[]; np: number[]; nq: number[]; sp: 1 | -1; sq: 1 | -1; ep?: number[][]; eq?: number[][]; Pp?: number; Pq?: number }
export interface Junction { x: number; y: number; unit: number; ways: Way[]; corners: Corner[] }

/** A road bridge: a way of the junctions at its ends, which its deck draws, not a corridor. */
export const isDeck = (line: Line) => line.kind === BRIDGE && !isPath(line) && line.code < RAIL_CODE && line.z.length >= 2;

/** Per line: alongs of its vertices, and what the plan does to it. */
interface LinePlan {
    alongs: number[];
    cuts: { lo: number; hi: number; bits: number }[];
    breaks: number[];
    /** A side arm's mouth: its start (end 0) or end (end 1), the alongs where its edges meet the through road, and its corners by side of the line. */
    mouths: { end: 0 | 1; m: number; far: number; P: Record<number, number[]>; J: Junction; edge: Record<number, Corner>; T?: JStation }[];
    /** A path's end on a road's outer edge: the along, and the height of the road's surface there. */
    ends: { at: number; road: Line; side: 1 | -1; walk: boolean; J: { x: number; y: number; unit: number } }[];
    /** Zebras on a road: their middle along it. */
    zebras: number[];
    /** Lowered kerbs: middle along it and sides. */
    lowered: { lo: number; hi: number; sides: number }[];
    /** Give-way lines on a side arm: along of the line's near edge (toward the junction), and its direction away from the junction. */
    giveWays: { at: number; dir: 1 | -1; reach: number }[];
    stations?: JStation[];
}

export interface Plan {
    junctions: Junction[];
    stations(line: Line): JStation[];
    faces(ctxOf: (line: Line) => Ctx | undefined, owns: Owns): void;
}

const plans = new WeakMap<Line[], Plan>();

export function junctionPlan(lines: Line[]): Plan {
    const known = plans.get(lines);
    if (known) return known;
    const plan = makePlan(lines);
    plans.set(lines, plan);
    return plan;
}

const dot = (p: number[], q: number[]) => p[0] * q[0] + p[1] * q[1];
const lerp = (a: number, b: number, t: number) => a + (b - a) * t;
const smooth = (t: number) => t * t * (3 - 2 * t);

function alongsOf(line: Line): number[] {
    const out = [0];
    for (let v = 0; v + 1 < line.z.length; v++) out.push(out[v] + Math.hypot(line.xy[2 * v + 2] - line.xy[2 * v], line.xy[2 * v + 3] - line.xy[2 * v + 1]) * metresPerUnit(line.xy[2 * v + 1]));
    return out;
}

function makePlan(lines: Line[]): Plan {
    const ids = new Map(lines.map((line, id) => [line, id] as const));
    const info = new Map<Line, LinePlan>();
    const planOf = (line: Line) => info.get(line) ?? info.set(line, { alongs: alongsOf(line), cuts: [], breaks: [], mouths: [], ends: [], zebras: [], lowered: [], giveWays: [] }).get(line)!;
    const usable = (line: Line) => corridorLine(line) && line.z.length >= 2 && (line.kind === ROAD || line.kind === PATH);
    const at = new Map<string, { line: Line; v: number }[]>();
    for (const line of lines) {
        const deck = isDeck(line);
        if (!usable(line) && !deck) continue;
        for (let v = 0; v < line.z.length; v++) {
            if ((deck && v > 0 && v + 1 < line.z.length) || !(line.anchors?.[v] ?? true)) continue;
            const key = `${line.xy[2 * v]},${line.xy[2 * v + 1]}`;
            (at.get(key) ?? at.set(key, []).get(key)!).push({ line, v });
        }
    }
    const junctions: Junction[] = [];
    const partners = partnersOf(lines), stamped = cornersOf(lines);
    for (const here of at.values()) {
        if (here.length < 2) continue;
        const x = here[0].line.xy[2 * here[0].v], y = here[0].line.xy[2 * here[0].v + 1], unit = metresPerUnit(y);
        const ways = here.flatMap(({ line, v }) => ([-1, 1] as const).filter(step => v + step >= 0 && v + step < line.z.length).map(step => wayOut(line, v, step, unit)));
        const roads = ways.filter(w => w.line.kind === ROAD || isDeck(w.line)), ground = roads.filter(w => w.line.kind === ROAD), paths = ways.filter(w => w.line.kind === PATH);
        if (roads.length >= 3 && ground.length >= 2) {
            const J = roadJunction(x, y, unit, roads, planOf, stamped);
            if (J) junctions.push(J);
        }
        if (ground.length >= 2 && paths.length >= 1) pathsAt(x, y, unit, ground, paths, planOf);
    }
    crossings(lines, usable, planOf);
    // Joins where OSM splits a road and no junction is: the two ends take the mitre of their bend.
    const mitres = new Map<Line, [number[]?, number[]?]>();
    for (const [line, pair] of partners) {
        if (!usable(line)) continue;
        pair.forEach((other, end) => {
            if (!other || !usable(other)) return;
            const n = line.z.length, v = end ? n - 1 : 0, w = end ? n - 2 : 1;
            const m = other.z.length, sx = line.xy[2 * v], sy = line.xy[2 * v + 1];
            const ov = other.xy[0] === sx && other.xy[1] === sy ? 0 : m - 1, ow = ov === 0 ? 1 : m - 2;
            // Directions along this line's own sense: in at its end, out at its start. The fit's tangents where
            // both lines have them, so the mitre is the stations' own direction where the fit runs on through.
            const away = other.dir ? (ov === 0 ? 1 : -1) : 0;
            const own = line.dir && away ? [line.dir[2 * v], line.dir[2 * v + 1]] : end ? [sx - line.xy[2 * w], sy - line.xy[2 * w + 1]] : [line.xy[2 * w] - sx, line.xy[2 * w + 1] - sy];
            const theirs = line.dir && other.dir ? [other.dir![2 * ov] * away * (end ? 1 : -1), other.dir![2 * ov + 1] * away * (end ? 1 : -1)]
                : end ? [other.xy[2 * ow] - sx, other.xy[2 * ow + 1] - sy] : [sx - other.xy[2 * ow], sy - other.xy[2 * ow + 1]];
            const lo = Math.hypot(own[0], own[1]) || 1, lt = Math.hypot(theirs[0], theirs[1]) || 1;
            const t = [own[0] / lo + theirs[0] / lt, own[1] / lo + theirs[1] / lt], l = Math.hypot(t[0], t[1]);
            if (l < 0.3) return;
            const cos = Math.max(0.5, dot(t, own) / l / lo);
            (mitres.get(line) ?? mitres.set(line, [undefined, undefined]).get(line)!)[end] = [t[0] / l / cos, t[1] / l / cos];
        });
    }

    const stations = (line: Line): JStation[] => {
        const lp = info.get(line);
        if (lp?.stations) return lp.stations;
        // The stations carry the junctions' planes (a side road's first metres on the through road's surface)
        // and the stamp's half-width: tapered to a partner's at a joint, widened in a tight bend.
        let list: JStation[] = stationsOf(line, undefined, v => halfAt(line, v, partners) + (line.widen?.[v] ?? 0)).map(s => ({ ...s }));
        const alongs = lp?.alongs ?? alongsOf(line);
        const ends = mitres.get(line);
        ends?.forEach((dir, end) => {
            if (!dir) return;
            const s = list[end ? list.length - 1 : 0], m = metresPerUnit(s.y);
            Object.assign(s, { ax: dir[1] / m, ay: -dir[0] / m, j: undefined });
        });
        if (lp) {
            const total = alongs[alongs.length - 1];
            for (const b of [...new Set(lp.breaks.map(b => Math.round(b * 1000) / 1000))].sort((p, q) => p - q)) {
                if (b <= 0.01 || b >= total - 0.01) continue;
                const k = list.findIndex(s => s.along >= b);
                if (k <= 0 || Math.abs(list[k].along - b) < 0.01 || Math.abs(list[k - 1].along - b) < 0.01) continue;
                list.splice(k, 0, mixStation(list[k - 1], list[k], (b - list[k - 1].along) / (list[k].along - list[k - 1].along)));
            }
            for (let k = 0; k + 1 < list.length; k++) {
                const mid = (list[k].along + list[k + 1].along) / 2;
                let bits = 0;
                for (const c of lp.cuts) if (mid > c.lo && mid < c.hi) bits |= c.bits;
                if (bits) list[k].cut = bits;
            }
        }
        if (lp) lp.stations = list;
        else info.set(line, { alongs, cuts: [], breaks: [], mouths: [], ends: [], zebras: [], lowered: [], giveWays: [], stations: list });
        return list;
    };

    // The mouths' own ends of the carriageway, and the paths' ends on the roads' edges, need the other lines' stations: made after.
    let finished = false;
    /** The first whole station of a side arm's carriageway past its mouth. */
    const mouthT = (w: Way) => info.get(w.line)?.mouths.find(m => m.end === (w.step > 0 ? 0 : 1) && m.J.ways.includes(w))?.T;
    const finish = () => {
        if (finished) return;
        finished = true;
        // A side arm's first metres lie on the through road's surface, banked as it is, so its mouth meets the
        // through road's edge level; the fit's heights hold the through road's middle only. Where that edge lies
        // under the side arm, it rises to it first.
        for (const [s, [left, right]] of raises(info, stations, ids)) {
            s.z += (left + right) / 2;
            s.bank += (left - right) / (2 * s.h);
        }
        for (const [line, lp] of info) for (const mouth of lp.mouths) conform(line, stations(line), lp.alongs, mouth, stations, ids, lines);
        for (const [line, lp] of info) {
            if (!lp.mouths.length && !lp.ends.length) continue;
            const list = stations(line);
            for (const mouth of lp.mouths) {
                // The carriageway's first station past both edges' meeting points: the mouth's surface runs from the through road's edge to it.
                let T = -1;
                if (mouth.end === 0) T = list.findIndex(s => s.along >= mouth.far - 1e-3);
                else for (let k = list.length - 1; k >= 0 && T < 0; k--) if (list[k].along <= mouth.far + 1e-3) T = k;
                if (T >= 0) mouth.T = list[T];
            }
            for (const end of lp.ends) {
                const k = list.findIndex(s => Math.abs(s.along - end.at) < 0.011);
                if (k < 0) continue;
                const s = list[k], z = surfaceAt(end.road, stations(end.road), ids, s.x, s.y);
                if (z === undefined) continue;
                // On the road's sidewalk, or its shoulder: the path's tread meets it level.
                s.z = z.edge + (end.walk ? KERB_RISE_M : -0.02) - LIFT_M - classLift(line, ids.get(line)!);
                s.j = undefined;
            }
        }
    };

    return {
        junctions,
        stations(line) {
            finish();
            return stations(line);
        },
        faces(ctxOf, owns) {
            finish();
            for (const J of junctions) {
                if (!owns(J.x, J.y)) continue;
                for (const c of J.corners) if (!isDeck(c.p.line) && !isDeck(c.q.line)) cornerFaces(J, c, ctxOf, stations, ids, mouthT);
            }
            for (const [line, lp] of info) {
                const ctx = ctxOf(line);
                if (ctx) for (const mouth of lp.mouths) if (mouth.T && owns(mouth.J.x, mouth.J.y)) mouthFaces(ctx, mouth, ctxOf, stations);
            }
            for (const [line, lp] of info) {
                const ctx = ctxOf(line);
                if (!ctx || !lp.stations) continue;
                markings(ctx, lp, owns);
            }
        },
    };
}

/** A station a share u of the way from s to t, made up. */
function mixStation(s: Station, t: Station, u: number): JStation {
    return {
        x: lerp(s.x, t.x, u), y: lerp(s.y, t.y, u), z: lerp(s.z, t.z, u), ax: lerp(s.ax, t.ax, u), ay: lerp(s.ay, t.ay, u), along: lerp(s.along, t.along, u), v: u < 1 ? s.v : t.v,
        bank: lerp(s.bank, t.bank, u), h: lerp(s.h, t.h, u), sleepers: s.sleepers === undefined || t.sleepers === undefined ? undefined : lerp(s.sleepers, t.sleepers, u),
    };
}

/** A way out of vertex v of a line: its direction toward its point about 8 m on, and the metres it runs within 0.3 m of straight, at most to the middle of the line's next junction or its end. */
function wayOut(line: Line, v: number, step: 1 | -1, unit: number): Way {
    const x = line.xy[2 * v], y = line.xy[2 * v + 1], n = line.z.length, REACH = 8;
    let k = v + step, length = 0, tx = x, ty = y;
    for (; k >= 0 && k < n; k += step) {
        const px = line.xy[2 * k - 2 * step], py = line.xy[2 * k + 1 - 2 * step], piece = Math.hypot(line.xy[2 * k] - px, line.xy[2 * k + 1] - py) * unit;
        const u = Math.min(1, (REACH - length) / (piece || 1));
        tx = px + u * (line.xy[2 * k] - px);
        ty = py + u * (line.xy[2 * k + 1] - py);
        if ((length += piece) >= REACH) break;
    }
    const dx = (tx - x) * unit, dy = (ty - y) * unit, l = Math.hypot(dx, dy) || 1, u: [number, number] = [dx / l, dy / l];
    let room = 0, total = 0;
    for (k = v + step; k >= 0 && k < n; k += step) {
        const ex = (line.xy[2 * k] - x) * unit, ey = (line.xy[2 * k + 1] - y) * unit;
        total = Math.hypot(ex, ey);
        if (Math.abs(ex * u[1] - ey * u[0]) > STRAIGHT_M || room >= 14) break;
        room = Math.min(14, ex * u[0] + ey * u[1]);
    }
    // The line's far end may be another junction: leave it half the line.
    const last = step > 0 ? n - 1 : 0, far = Math.hypot((line.xy[2 * last] - x) * unit, (line.xy[2 * last + 1] - y) * unit);
    room = Math.min(room, Math.max(total, far) * 0.45);
    return { line, v, step, u, angle: Math.atan2(u[1], u[0]), room };
}

/** The half-width of a way near its junction. */
const halfOf = (w: Way) => w.line.h;
/** A way's segment next to its junction. */
const segOf = (w: Way) => (w.step > 0 ? w.v : w.v - 1);

/** A junction of three or more road ways: its through pair, its side arms and corners, and their cuts. */
function roadJunction(x: number, y: number, unit: number, all: Way[], planOf: (line: Line) => LinePlan, stamped: StampCorner[]): Junction | undefined {
    let ways = all.sort((p, q) => p.angle - q.angle);
    // The through pair: the best class, then the straightest.
    let pair: [Way, Way] | undefined, best = [Infinity, Infinity, Infinity];
    for (let i = 0; i < ways.length; i++) for (let k = i + 1; k < ways.length; k++) {
        const c = dot(ways[i].u, ways[k].u);
        if (c > THROUGH_COS) continue;
        const rank = [Math.min(ways[i].line.profile, ways[k].line.profile), Math.max(ways[i].line.profile, ways[k].line.profile), c];
        if (rank[0] < best[0] || (rank[0] === best[0] && (rank[1] < best[1] || (rank[1] === best[1] && rank[2] < best[2])))) [best, pair] = [rank, [ways[i], ways[k]]];
    }
    if (!pair) return undefined;
    for (const w of ways) w.through = pair.includes(w);
    // A driveway (a service road or a track without sidewalks) crosses the sidewalk of a town road on a lowered kerb.
    const drives = ways.filter(w => !w.through && DRIVES.includes(CLASS_NAMES[w.line.profile]) && !w.line.sidewalks?.some(Boolean) && !pair!.some(p => isDeck(p.line)) && pair!.some(p => kerbedToward(p, w)));
    if (drives.length) {
        ways = ways.filter(w => !drives.includes(w));
        pathsAt(x, y, unit, ways, drives, planOf);
        for (const d of drives) for (const p of pair) {
            if (!kerbedToward(p, d)) continue;
            const lp = planOf(p.line), base = lp.alongs[p.v], sin = Math.max(0.3, Math.abs(p.u[0] * d.u[1] - p.u[1] * d.u[0])), reach = (d.line.h + 0.5) / sin;
            lower(lp, base - reach, base + reach, sideToward(p, d) > 0 ? 1 : 2, true);
        }
    }
    const J: Junction = { x, y, unit, ways, corners: [] };
    const n = ways.length;
    for (let i = 0; i < n; i++) {
        const p = ways[i], q = ways[(i + 1) % n];
        const turn = q.angle - p.angle + (i + 1 === n ? 2 * Math.PI : 0);
        const sideCorner = !p.through || !q.through;
        if (turn >= (sideCorner ? MAX_SIDE_TURN : MAX_TURN)) continue;
        // Two side arms next to each other: neither is trimmed against the other.
        if (!p.through && !q.through) continue;
        const c = cornerOf(p, q);
        if (!c) continue;
        // Where the stamp rounds this corner (paved roads), take its arc, so the ground under the fillet is held flat.
        const s = stampCorner(stamped, J, c);
        if (s) Object.assign(c, s);
        // The edges' meeting point on the ways' own curves, not on their straight lines.
        c.ep = edgeOf(p, c.sp, J);
        c.eq = edgeOf(q, c.sq, J);
        const P = meet(c.ep, c.eq);
        if (P && Math.hypot(P[0] - c.P[0], P[1] - c.P[1]) < 2) {
            if (c.r === 0) c.a = c.b = c.o = P;
            c.P = P;
        }
        // A deck's edge has its kerb or parapet: no arc rounds it.
        if (isDeck(p.line) || isDeck(q.line)) Object.assign(c, { r: 0, a: c.P, b: c.P, o: c.P });
        J.corners.push(c);
    }
    // A side arm whose both neighbours are corners with the through road gets its mouth.
    for (const s of ways) {
        if (s.through) continue;
        const left = J.corners.find(c => c.q === s), right = J.corners.find(c => c.p === s);
        s.side = !isDeck(s.line) && !!left && !!right;
    }
    J.corners = J.corners.filter(c => (c.p.through || c.p.side) && (c.q.through || c.q.side));
    for (const c of J.corners) {
        for (const [w, end, sideOfLine] of [[c.p, c.a, c.sp], [c.q, c.b, c.sq]] as const) {
            const lp = planOf(w.line), base = lp.alongs[w.v], t = dot(end, w.u), at = base + w.step * t;
            lp.cuts.push({ lo: Math.min(base, at), hi: Math.max(base, at), bits: sideOfLine > 0 ? CUT.LEFT : CUT.RIGHT });
            lp.breaks.push(at);
            // A station where the edges meet, on a through way: the fillet and the mouth share its edge vertex.
            if (w.through) {
                const along = base + w.step * Math.max(0, dot(c.P, w.u));
                lp.breaks.push(along);
                if (w === c.p) c.Pp = along;
                else c.Pq = along;
            }
        }
    }
    for (const s of ways) {
        if (!s.side) continue;
        const cl = J.corners.find(c => c.q === s)!, cr = J.corners.find(c => c.p === s)!;
        // The edges' meeting points with the through road, by side of the side arm's line.
        const P: Record<number, number[]> = { [cl.sq]: cl.P, [cr.sp]: cr.P }, edge: Record<number, Corner> = { [cl.sq]: cl, [cr.sp]: cr };
        if (P[1] === undefined || P[-1] === undefined) continue;
        const m = Math.max(dot(P[1], s.u), dot(P[-1], s.u));
        const lp = planOf(s.line), base = lp.alongs[s.v], total = lp.alongs[lp.alongs.length - 1];
        const length = s.step > 0 ? total - base : base;
        if (m + 1 > length) continue;
        const far = base + s.step * m;
        lp.mouths.push({ end: s.step > 0 ? 0 : 1, m: base + s.step * (dot(P[1], s.u) + dot(P[-1], s.u)) / 2, far, P, J, edge });
        lp.cuts.push({ lo: Math.min(base, far), hi: Math.max(base, far), bits: CUT.ROAD });
        lp.breaks.push(far);
        // A minor paved road meets a main road: a give-way line across its mouth.
        const main = pairClass(ways);
        if (main && MAIN.includes(main) && PAVED_CODES.includes(s.line.code) && !MAIN.slice(0, MAIN.indexOf(main) + 2).includes(CLASS_NAMES[s.line.profile])) {
            const g = far + s.step * GIVE_WAY_BACK_M;
            // Only a triangle that fits: a short arm keeps the line alone.
            const reach = m + GIVE_WAY_BACK_M + GIVE_WAY_REACH_M + 2 < length ? GIVE_WAY_REACH_M : GIVE_WAY_M;
            if (m + GIVE_WAY_BACK_M + reach + 1 < length) {
                lp.giveWays.push({ at: g, dir: s.step, reach });
                lp.breaks.push(g, g + s.step * reach);
                lp.cuts.push({ lo: Math.min(g, g + s.step * reach), hi: Math.max(g, g + s.step * reach), bits: CUT.ROAD });
            }
        }
    }
    return J;
}

/** The classes that cross a town road's sidewalk as a driveway. */
const DRIVES = ['service', 'track'];
/** The side of way p's line (1 left, -1 right) that way w leaves the junction on. */
const sideToward = (p: Way, w: Way) => {
    const d = [p.step * p.u[0], p.step * p.u[1]];
    return w.u[0] * d[1] - w.u[1] * d[0] > 0 ? 1 : -1;
};
const kerbedToward = (p: Way, w: Way) => sideOf(p.line, sideToward(p, w) as 1 | -1, segOf(p)).some(part => part.mat === MAT.KERB);

const pairClass = (ways: Way[]) => {
    const through = ways.filter(w => w.through);
    return through.length ? CLASS_NAMES[Math.min(...through.map(w => w.line.profile))] : undefined;
};

/** The corner from way p to the next way q round the junction, or a sharp one where no arc fits. */
function cornerOf(p: Way, q: Way): Corner | undefined {
    const np = [-p.u[1], p.u[0]], nq = [q.u[1], -q.u[0]], det = np[0] * nq[1] - np[1] * nq[0];
    if (Math.abs(det) < 1e-3) return undefined;
    const X = (d: number) => {
        const dp = halfOf(p) + d, dq = halfOf(q) + d;
        return [(dp * nq[1] - dq * np[1]) / det, (dq * np[0] - dp * nq[0]) / det];
    };
    const P = X(0);
    if (dot(P, p.u) < -0.5 || dot(P, q.u) < -0.5) return undefined;
    const paved = (w: Way) => PAVED_CODES.includes(w.line.code);
    const town = !!(p.line.sidewalks?.some(Boolean) || q.line.sidewalks?.some(Boolean));
    let r = paved(p) && paved(q) ? (town ? TOWN_R : RURAL_R) : TRACK_R;
    // The arc's ends stay within each way's room.
    for (const [w, n] of [[p, np], [q, nq]] as const) {
        const a0 = dot([X(0)[0] - 0, X(0)[1]], w.u), slope = dot([X(1)[0] - n[0], X(1)[1] - n[1]], w.u) - a0;
        if (slope > 1e-6 && a0 + slope * r > w.room) r = Math.max(0, (w.room - a0) / slope);
    }
    if (r < LEAST_R) r = 0;
    const sp = p.step > 0 ? -1 : 1, sq = q.step > 0 ? 1 : -1;
    // A kerbed corner keeps its sidewalk round the arc: its radius at least the sidewalk's width.
    const walk = Math.max(...([[p, sp], [q, sq]] as const).map(([w, side]) => {
        const parts = sideOf(w.line, side, segOf(w));
        return parts.some(part => part.mat === MAT.KERB) ? parts.filter(part => part.mat !== MAT.GROUND).at(-1)!.a - w.line.h : 0;
    }));
    if (walk > 0) r = Math.max(r, walk + 0.6);
    const o = X(r), a = [o[0] - r * np[0], o[1] - r * np[1]], b = [o[0] - r * nq[0], o[1] - r * nq[1]];
    // n_p is p's right seen outward; n_q is q's left.
    return { p, q, o, r, a, b, P, np, nq, sp, sq };
}

/** The stamp's corner of a corner's two ways, as metres about the junction: its arc's centre and ends and its radius, if it has one. */
function stampCorner(stamped: StampCorner[], J: { x: number; y: number; unit: number }, c: Corner): Partial<Corner> | undefined {
    const local = (p: number[]) => [(p[0] - J.x) * J.unit, (p[1] - J.y) * J.unit];
    // An arc end on a way's edge: about its half-width off its line and ahead of the junction.
    const onEdge = (pt: number[], w: Way) => Math.abs(Math.abs(pt[0] * -w.u[1] + pt[1] * w.u[0]) - w.line.h) < 0.6 && dot(pt, w.u) > -0.5 && dot(pt, w.u) < 16;
    for (const sc of stamped) {
        const a = local(sc.a), b = local(sc.b), o = local(sc.o);
        if (Math.hypot(o[0], o[1]) > 25 || !onEdge(a, c.p) || !onEdge(b, c.q)) continue;
        const r = Math.hypot(a[0] - o[0], a[1] - o[1]);
        if (r < 0.5) continue;
        return { o, a, b, r, np: [(o[0] - a[0]) / r, (o[1] - a[1]) / r], nq: [(o[0] - b[0]) / r, (o[1] - b[1]) / r] };
    }
    return undefined;
}

/** A way's edge on its line's side `side`, from its junction out about 25 m, in metres about the junction. */
function edgeOf(w: Way, side: 1 | -1, J: { x: number; y: number; unit: number }): number[][] {
    const { line } = w, n = line.z.length, out: number[][] = [];
    for (let k = w.v, dist = 0; k >= 0 && k < n && dist < 25; k += w.step) {
        const a = Math.max(0, k - 1), b = Math.min(n - 1, k + 1), dx = line.xy[2 * b] - line.xy[2 * a], dy = line.xy[2 * b + 1] - line.xy[2 * a + 1], l = Math.hypot(dx, dy) || 1;
        const p = [(line.xy[2 * k] - J.x) * J.unit + (dy / l) * side * line.h, (line.xy[2 * k + 1] - J.y) * J.unit - (dx / l) * side * line.h];
        if (out.length) dist += Math.hypot(p[0] - out[out.length - 1][0], p[1] - out[out.length - 1][1]);
        out.push(p);
    }
    return out;
}

/** The first point where polyline q crosses polyline p, by its distance along p. */
function meet(p: number[][], q: number[][]): number[] | undefined {
    for (let i = 0; i + 1 < p.length; i++) {
        let best: number[] | undefined, least = Infinity;
        for (let k = 0; k + 1 < q.length; k++) {
            const [a, b, c, d] = [p[i], p[i + 1], q[k], q[k + 1]], r = [b[0] - a[0], b[1] - a[1]], s = [d[0] - c[0], d[1] - c[1]], den = r[0] * s[1] - r[1] * s[0];
            if (Math.abs(den) < 1e-9) continue;
            const t = ((c[0] - a[0]) * s[1] - (c[1] - a[1]) * s[0]) / den, u = ((c[0] - a[0]) * r[1] - (c[1] - a[1]) * r[0]) / den;
            if (t >= 0 && t <= 1 && u >= 0 && u <= 1 && t < least) [least, best] = [t, [a[0] + t * r[0], a[1] + t * r[1]]];
        }
        if (best) return best;
    }
    return undefined;
}

/** The paths that meet roads at a vertex: each stops at the roads' outer edges; a path that runs on across a paved town road crosses it on a zebra. */
function pathsAt(x: number, y: number, unit: number, roads: Way[], paths: Way[], planOf: (line: Line) => LinePlan): void {
    // The roads' cores as strips from the vertex out along each way: half-width and side widths.
    const core = (w: Way, side: 1 | -1) => {
        const parts = sideOf(w.line, side, segOf(w));
        const solid = parts.filter(p => p.mat !== MAT.GROUND);
        return { out: solid.length ? solid[solid.length - 1].a : w.line.h, walk: parts.some(p => p.mat === MAT.SIDEWALK) };
    };
    const inside = (pt: number[]) => roads.some(w => {
        const t = dot(pt, w.u), lat = pt[0] * -w.u[1] + pt[1] * w.u[0];
        // lat > 0 is the way's outward left (y runs south): its line's left when it steps forward.
        const lineSide = (lat > 0 ? 1 : -1) * (w.step > 0 ? 1 : -1) as 1 | -1;
        return t > -w.line.h - 0.5 && Math.abs(lat) < core(w, lineSide).out;
    });
    const exits: { w: Way; t: number }[] = [];
    for (const w of paths) {
        const h = w.line.h, n = [-w.u[1], w.u[0]];
        let t = 0;
        while (t < 25 && [-h, 0, h].some(s => inside([w.u[0] * t + n[0] * s, w.u[1] * t + n[1] * s]))) t += 0.1;
        if (t >= 25) continue;
        const lp = planOf(w.line), base = lp.alongs[w.v], total = lp.alongs[lp.alongs.length - 1], at = base + w.step * t;
        if (at <= 0.2 || at >= total - 0.2) continue;
        // The road that the path leaves last: the nearest way to the exit point.
        const pt = [w.u[0] * t, w.u[1] * t];
        let road = roads[0], least = Infinity;
        for (const r of roads) {
            const lat = Math.abs(pt[0] * -r.u[1] + pt[1] * r.u[0]);
            if (dot(pt, r.u) > -1 && lat < least) [least, road] = [lat, r];
        }
        const latSign = pt[0] * -road.u[1] + pt[1] * road.u[0] > 0 ? 1 : -1, lineSide = (latSign * road.step) as 1 | -1;
        lp.cuts.push({ lo: Math.min(base, at), hi: Math.max(base, at), bits: CUT.ALL });
        lp.breaks.push(at);
        lp.ends.push({ at, road: road.line, side: lineSide, walk: core(road, lineSide).walk, J: { x, y, unit } });
        exits.push({ w, t });
    }
    // A path that runs on across: two of its ways on opposite sides of the road.
    const through = roads.filter(w => w.line.code && PAVED_CODES.includes(w.line.code));
    if (through.length < 2) return;
    for (let i = 0; i < exits.length; i++) for (let k = i + 1; k < exits.length; k++) {
        const [p, q] = [exits[i].w, exits[k].w];
        if (dot(p.u, q.u) > -0.5 || !ZEBRA_PATHS.includes(CLASS_NAMES[p.line.profile])) continue;
        const town = p.line.code === PAVING_CODE || q.line.code === PAVING_CODE || through.some(w => w.line.sidewalks?.some(Boolean));
        if (!town) continue;
        // The zebra runs ZEBRA_HALF_M each way along the road from the vertex.
        for (const r of through) zebraOn(planOf(r.line), r.v, r.step);
    }
}

/** A zebra on road `line`, its middle at vertex v, on the half that runs from it by `step` (both halves when called for both ways). */
function zebraOn(lp: LinePlan, v: number, step: 1 | -1): void {
    const base = lp.alongs[v], total = lp.alongs[lp.alongs.length - 1];
    if (base - ZEBRA_HALF_M - RAMP_M < 0 && step < 0) return;
    if (base + ZEBRA_HALF_M + RAMP_M > total && step > 0) return;
    zebraAt(lp, base, step);
}

function zebraAt(lp: LinePlan, at: number, step: 0 | 1 | -1): void {
    const total = lp.alongs[lp.alongs.length - 1];
    const lo = step > 0 ? at : Math.max(0, at - ZEBRA_HALF_M), hi = step < 0 ? at : Math.min(total, at + ZEBRA_HALF_M);
    if (!lp.zebras.includes(at)) lp.zebras.push(at);
    lp.breaks.push(lo, hi);
    lp.cuts.push({ lo, hi, bits: CUT.ROAD });
    lower(lp, at - ZEBRA_HALF_M, at + ZEBRA_HALF_M, 3, true);
}

/** A lowered kerb on a road from along lo to hi on its sides (1 left, 2 right), ramped back up past them; its sides are cut there for the lowered faces. */
function lower(lp: LinePlan, lo: number, hi: number, sides: number, kerbed: boolean): void {
    if (!kerbed || lp.lowered.some(l => Math.abs(l.lo - lo) < 0.01 && l.sides === sides)) return;
    lp.lowered.push({ lo, hi, sides });
    lp.breaks.push(lo, hi, lo - RAMP_M, hi + RAMP_M);
    lp.cuts.push({ lo: lo - RAMP_M, hi: hi + RAMP_M, bits: (sides & 1 ? CUT.LEFT : 0) | (sides & 2 ? CUT.RIGHT : 0) });
}

/** Paths that cross a paved town road between vertices: their segments cross the road's segments. */
function crossings(lines: Line[], usable: (line: Line) => boolean, planOf: (line: Line) => LinePlan): void {
    const roads = lines.filter(l => usable(l) && l.kind === ROAD && PAVED_CODES.includes(l.code) && l.sidewalks?.some(Boolean));
    const segs: [Line, number][] = [], boxes: number[][] = [];
    for (const line of roads) for (let v = 0; v + 1 < line.z.length; v++) {
        const [ax, ay, bx, by] = line.xy.slice(2 * v, 2 * v + 4);
        segs.push([line, v]);
        boxes.push([Math.min(ax, bx), Math.min(ay, by), Math.max(ax, bx), Math.max(ay, by)]);
    }
    if (!segs.length) return;
    const grid = gridOf(boxes);
    for (const path of lines) {
        if (!usable(path) || path.kind !== PATH || !ZEBRA_PATHS.includes(CLASS_NAMES[path.profile])) continue;
        for (let v = 0; v + 1 < path.z.length; v++) {
            const [px, py, qx, qy] = path.xy.slice(2 * v, 2 * v + 4);
            near(grid, px, py, qx, qy, 0, k => {
                const [line, w] = segs[k], [ax, ay, bx, by] = line.xy.slice(2 * w, 2 * w + 4);
                const d = (qx - px) * (by - ay) - (qy - py) * (bx - ax);
                if (Math.abs(d) < 1e-9) return;
                const t = ((ax - px) * (by - ay) - (ay - py) * (bx - ax)) / d, u = ((ax - px) * (qy - py) - (ay - py) * (qx - px)) / d;
                // Strictly inside both segments: a shared vertex is a junction, handled there.
                if (t <= 1e-6 || t >= 1 - 1e-6 || u <= 1e-6 || u >= 1 - 1e-6) return;
                const unit = metresPerUnit(py), pl = Math.hypot(qx - px, qy - py) * unit, rl = Math.hypot(bx - ax, by - ay) * unit;
                const cos = Math.abs(((qx - px) * (bx - ax) + (qy - py) * (by - ay)) * unit * unit / (pl * rl || 1));
                if (cos > 0.6) return;
                const sin = Math.sqrt(1 - cos * cos), rp = planOf(line), pp = planOf(path);
                // The path stops at the road's outer edges each side.
                const out = Math.max(...([1, -1] as const).map(side => {
                    const solid = sideOf(line, side, w).filter(p => p.mat !== MAT.GROUND);
                    return solid.length ? solid[solid.length - 1].a : line.h;
                }));
                const reach = out / sin + path.h * cos / sin, mid = pp.alongs[v] + t * pl;
                if (mid - reach < 0.3 || mid + reach > pp.alongs[pp.alongs.length - 1] - 0.3) return;
                pp.cuts.push({ lo: mid - reach, hi: mid + reach, bits: CUT.ALL });
                pp.breaks.push(mid - reach, mid + reach);
                const at = rp.alongs[w] + u * rl;
                if (at - ZEBRA_HALF_M - RAMP_M > 0 && at + ZEBRA_HALF_M + RAMP_M < rp.alongs[rp.alongs.length - 1]) zebraAt(rp, at, 0);
            });
        }
    }
}

/** The height of a road's surface at a point near it, from its stations: the edge's height and its along at the foot. */
function surfaceAt(line: Line, list: Station[], ids: Map<Line, number>, x: number, y: number): { edge: number; along: number; lat: number } | undefined {
    let best: { edge: number; along: number; lat: number } | undefined, least = Infinity;
    const unit = metresPerUnit(y);
    for (let k = 0; k + 1 < list.length; k++) {
        const s = list[k], t = list[k + 1], dx = (t.x - s.x) * unit, dy = (t.y - s.y) * unit, l2 = dx * dx + dy * dy || 1;
        const ex = (x - s.x) * unit, ey = (y - s.y) * unit, u = Math.max(0, Math.min(1, (ex * dx + ey * dy) / l2));
        const d = Math.hypot(ex - u * dx, ey - u * dy);
        if (d >= least) continue;
        least = d;
        const lat = (ex * dy - ey * dx) / Math.sqrt(l2), side = Math.sign(lat) || 1;
        const bank = lerp(s.bank, t.bank, u), edge = lerp(s.z, t.z, u) + LIFT_M + bank * side * line.h + classLift(line, ids.get(line) ?? 0);
        best = { edge, along: lerp(s.along, t.along, u), lat };
    }
    return best;
}

/** A side arm leaves the through road's surface over this many metres past its mouth. */
const CONFORM_M = 10;
/**
 * The stamped ground holds a side arm at its own heights: its surface sinks at most SINK_M under them. Where the
 * through road's banked edge lies lower, that edge rises (at most RAISE_M) about its other edge instead, fully
 * within FLAT_M of the junction and back over FLATTEN_M more.
 */
const SINK_M = 0.05, RAISE_M = 0.3, FLAT_M = 4, FLATTEN_M = 10;

/** The height of a road's surface at a point near it, from its stations: its banked plane, held level past its edges. */
function planeAt(line: Line, list: Station[], ids: Map<Line, number>, x: number, y: number): { z: number; lat: number } | undefined {
    let best: { z: number; lat: number } | undefined, least = Infinity;
    const unit = metresPerUnit(y);
    for (let k = 0; k + 1 < list.length; k++) {
        const s = list[k], t = list[k + 1], dx = (t.x - s.x) * unit, dy = (t.y - s.y) * unit, l2 = dx * dx + dy * dy || 1;
        const ex = (x - s.x) * unit, ey = (y - s.y) * unit, u = Math.max(0, Math.min(1, (ex * dx + ey * dy) / l2));
        const d = Math.hypot(ex - u * dx, ey - u * dy);
        if (d >= least) continue;
        least = d;
        const lat = (ex * dy - ey * dx) / Math.sqrt(l2), h = lerp(s.h, t.h, u);
        best = { z: lerp(s.z, t.z, u) + LIFT_M + lerp(s.bank, t.bank, u) * Math.max(-h, Math.min(h, lat)) + classLift(line, ids.get(line) ?? 0), lat };
    }
    return best;
}

/** A mouth's side arm: the along of its junction's station, its direction away from it, and its station at the mouth's far end. */
function armOf(line: Line, list: JStation[], alongs: number[], mouth: LinePlan['mouths'][number]) {
    // The junction's station by its place, and the mouth's metres from it on the plan's chords.
    const { x: jx, y: jy } = mouth.J, base = list.reduce((a, s) => (Math.hypot(s.x - jx, s.y - jy) < Math.hypot(a.x - jx, a.y - jy) ? s : a), list[0]).along;
    const dir = mouth.end === 0 ? 1 : -1, v = Array.from({ length: line.z.length }, (_, k) => k).find(k => line.xy[2 * k] === jx && line.xy[2 * k + 1] === jy) ?? 0;
    const far = Math.abs(mouth.far - alongs[v]), at = (d: number) => list.reduce((a, s) => (Math.abs((s.along - base) * dir - d) < Math.abs((a.along - base) * dir - d) ? s : a), list[0]);
    return { base, dir, far, at };
}

/** The rise of each through road's edges near the mouths whose side arms lie higher than them: per station, left and right. */
function raises(info: Map<Line, LinePlan>, stations: (line: Line) => JStation[], ids: Map<Line, number>): Map<Station, [number, number]> {
    const out = new Map<Station, [number, number]>();
    for (const [line, lp] of info) for (const mouth of lp.mouths) {
        const list = stations(line), arm = armOf(line, list, lp.alongs, mouth), at = arm.at(arm.far), lift = classLift(line, ids.get(line) ?? 0);
        for (const t of new Set(mouth.J.ways.filter(w => w.through && !isDeck(w.line)).map(w => w.line))) {
            const tl = stations(t), p = planeAt(t, tl, ids, at.x, at.y);
            const need = p && at.z + LIFT_M + lift - p.z;
            if (!p || !need || need <= SINK_M) continue;
            const rise = Math.min(RAISE_M, need), side = p.lat > 0 ? 0 : 1, { x: jx, y: jy } = mouth.J;
            const tb = tl.reduce((a, s) => (Math.hypot(s.x - jx, s.y - jy) < Math.hypot(a.x - jx, a.y - jy) ? s : a), tl[0]).along;
            for (const s of tl) {
                const w = 1 - smooth(Math.max(0, Math.min(1, (Math.abs(s.along - tb) - FLAT_M) / FLATTEN_M)));
                if (w <= 0) continue;
                const r = out.get(s) ?? out.set(s, [0, 0]).get(s)!;
                r[side] = Math.max(r[side], rise * w);
            }
        }
    }
    return out;
}

/**
 * Moves a side arm's stations at a mouth onto the through roads' surface, and back to its own heights over
 * CONFORM_M past the mouth. Where its core still lies on a through deck's footprint, it stays on the deck's level.
 */
function conform(line: Line, list: JStation[], alongs: number[], mouth: LinePlan['mouths'][number], stations: (line: Line) => JStation[], ids: Map<Line, number>, lines: Line[]): void {
    const through = [...new Set(mouth.J.ways.filter(w => w.through).map(w => w.line))], lift = classLift(line, ids.get(line) ?? 0);
    const arm = armOf(line, list, alongs, mouth), { base, dir } = arm;
    const target = (s: Station) => {
        let best: { z: number; lat: number } | undefined;
        for (const t of through) {
            const p = planeAt(t, stations(t), ids, s.x, s.y);
            if (p && (!best || Math.abs(p.lat) < Math.abs(best.lat))) best = p;
        }
        return best && best.z - LIFT_M - lift;
    };
    const decks = through.filter(isDeck).map(t => ({ list: stations(t), outer: deckOuter(t, lines) })), core = line.h + 0.6;
    let far = arm.far;
    for (const s of dir > 0 ? list : [...list].reverse()) {
        const d = (s.along - base) * dir;
        if (d > far && d < far + CONFORM_M && decks.some(k => [-core, 0, core].some(a => onDeck(k.list, k.outer, s.x + s.ax * a, s.y + s.ay * a)))) far = d;
    }
    const at = arm.at(far), zFar = target(at);
    if (zFar === undefined) return;
    const shift = Math.max(-SINK_M, Math.min(0.5, zFar - at.z));
    for (const s of list) {
        const d = (s.along - base) * dir;
        if (d < 0 || d > far + CONFORM_M) continue;
        const z = d <= far ? target(s) : undefined;
        s.z = z !== undefined ? Math.max(z, s.z - SINK_M) : s.z + shift * (1 - smooth(Math.min(1, (d - far) / CONFORM_M)));
    }
}

/** Whether a point lies on a deck between its ends and within its outer edges (left, right), by its stations. */
function onDeck(list: Station[], outer: [number, number], x: number, y: number): boolean {
    const unit = metresPerUnit(y);
    for (let k = 0; k + 1 < list.length; k++) {
        const s = list[k], t = list[k + 1], dx = (t.x - s.x) * unit, dy = (t.y - s.y) * unit, l2 = dx * dx + dy * dy || 1;
        const ex = (x - s.x) * unit, ey = (y - s.y) * unit, u = (ex * dx + ey * dy) / l2;
        if (u < 0 || u > 1) continue;
        const lat = (ex * dy - ey * dx) / Math.sqrt(l2);
        if (lat <= outer[0] && -lat <= outer[1]) return true;
    }
    return false;
}

/** The world point of a junction's local metres. */
const world = (J: { x: number; y: number; unit: number }, p: number[]) => [J.x + p[0] / J.unit, J.y + p[1] / J.unit];

/** A station of a planned line at an along, made where the plan broke the line. */
function stationAt(list: JStation[], along: number): JStation | undefined {
    let best: JStation | undefined, least = 0.02;
    for (const s of list) if (Math.abs(s.along - along) < least) [least, best] = [Math.abs(s.along - along), s];
    return best;
}

/**
 * The parts of a corner's ring at a way's end: metres out from the edge, up, material and flags. A side
 * without a kerb maps onto a kerbed one's parts (a kerb flush with the shoulder), so the two ends interpolate.
 */
function ringParts(line: Line, side: 1 | -1, v: number, solidCount: number): Part[] {
    const parts = sideOf(line, side, v).map(p => ({ ...p, a: p.a - line.h }));
    const bands = parts.filter(p => p.mat === MAT.GROUND), solid = parts.filter(p => p.mat !== MAT.GROUND);
    if (solid.length === solidCount) return parts;
    const end = solid.length ? solid[solid.length - 1] : { a: 0, dz: LIFT_M, mat: MAT.SHOULDER };
    if (solidCount === 3) return [{ a: 0, dz: LIFT_M, mat: MAT.KERB }, { a: end.a * 0.5, dz: (LIFT_M + end.dz) / 2, mat: MAT.KERB }, { a: end.a, dz: end.dz, mat: MAT.SIDEWALK }, ...bands];
    return [...Array.from({ length: solidCount - solid.length }, () => ({ a: 0, dz: LIFT_M, mat: MAT.SHOULDER })), ...solid, ...bands];
}

/** A vertex with its height (a band's is an offset over the ground). */
type W = V & { z: number };

/** A quad, wound to face `want` (east, north, up): by its plan where it has area, else by its true normal. */
function faceToward(b: MeshBuilder, q: W[], want: number[]): void {
    const m = metresPerUnit(q[0].y), P = q.map(v => [(v.x - q[0].x) * m, -(v.y - q[0].y) * m, v.z]);
    const u = [P[2][0] - P[0][0], P[2][1] - P[0][1], P[2][2] - P[0][2]], w = [P[3][0] - P[1][0], P[3][1] - P[1][1], P[3][2] - P[1][2]];
    const n = [u[1] * w[2] - u[2] * w[1], u[2] * w[0] - u[0] * w[2], u[0] * w[1] - u[1] * w[0]];
    const flat = Math.abs(n[2]) > 1e-3;
    const facing = flat ? n[2] > 0 : n[0] * want[0] + n[1] * want[1] + n[2] * want[2] >= 0;
    if (facing) b.face(q[0].i, q[1].i, q[2].i, q[3].i);
    else b.face(q[0].i, q[3].i, q[2].i, q[1].i);
}

/** The faces of a corner: the asphalt fillet between the edges and the arc, then the kerb, the sidewalk and the band round the arc. */
function cornerFaces(J: Junction, c: Corner, ctxOf: (line: Line) => Ctx | undefined, stations: (line: Line) => JStation[], ids: Map<Line, number>, mouthT: (w: Way) => Station | undefined): void {
    const cp = ctxOf(c.p.line), cq = ctxOf(c.q.line);
    if (!cp || !cq) return;
    const lpA = stations(c.p.line), lpB = stations(c.q.line);
    const alongA = alongOfVertex(c.p.line, c.p.v) + c.p.step * dot(c.a, c.p.u), alongB = alongOfVertex(c.q.line, c.q.v) + c.q.step * dot(c.b, c.q.u);
    const sa = stationAt(lpA, alongA), sb = stationAt(lpB, alongB);
    if (!sa || !sb) return;
    const b = cp.b;
    const solidOf = (line: Line, side: 1 | -1, v: number) => sideOf(line, side, v).filter(p => p.mat !== MAT.GROUND).length;
    const na = solidOf(c.p.line, c.sp, sa.v), nb = solidOf(c.q.line, c.sq, sb.v), ka = na === 3, solid = Math.max(na, nb);
    const A = ringParts(c.p.line, c.sp, sa.v, solid), B = ringParts(c.q.line, c.sq, sb.v, solid);
    if (A.length !== B.length) return;
    const zOf = (ctx: Ctx, s: Station, a: number, dz: number) => s.z + dz + s.bank * a + ctx.lift;
    // Columns from the arc's end on p (f = 0) to its end on q (f = 1).
    const u0 = [-c.np[0], -c.np[1]], u1 = [-c.nq[0], -c.nq[1]], sweep = Math.atan2(u0[0] * u1[1] - u0[1] * u1[0], dot(u0, u1)), t0 = Math.atan2(u0[1], u0[0]);
    const ring = (d: number, f: number) => {
        if (d < c.r) {
            const t = t0 + sweep * f;
            return [c.o[0] + (c.r - d) * Math.cos(t), c.o[1] + (c.r - d) * Math.sin(t)];
        }
        // Past the arc's centre the ways' own sides meet: the ring ends there.
        return c.o;
    };
    // The ring's parts at f, with a ring at the arc's centre where the parts pass it.
    const partsAt = (f: number): Part[] => A.map((pa, i) => ({ a: lerp(pa.a, B[i].a, f), dz: lerp(pa.dz, B[i].dz, f), mat: (ka ? pa : B[i]).mat, flags: pa.flags }));
    const za = zOf(cp, sa, c.sp * sa.h, LIFT_M), zb = zOf(cq, sb, c.sq * sb.h, LIFT_M);
    const prio = Math.max(cp.prio, cq.prio), code = c.p.through ? c.p.line.code : c.q.through ? c.q.line.code : Math.min(c.p.line.code, c.q.line.code);
    const half_ = Math.min(c.p.line.h, c.q.line.h);
    // A ring's vertex at f in material `mat`: at the ends, the ways' own (so the faces meet theirs exactly); between, the arc's.
    // The ways' edges curve away from the straight lines that the arc was fitted to: the arc's columns shift
    // by the miss at each end, blended along it, so its ends meet the ways' own sections.
    const local = (s: Station, a: number) => [(s.x + s.ax * a - J.x) * J.unit, (s.y + s.ay * a - J.y) * J.unit];
    const la = local(sa, c.sp * sa.h), lb = local(sb, c.sq * sb.h), e0 = [la[0] - c.a[0], la[1] - c.a[1]], e1 = [lb[0] - c.b[0], lb[1] - c.b[1]];
    const shifted = (p: number[], f: number) => {
        const t = smooth(f);
        return [p[0] + lerp(e0[0], e1[0], t), p[1] + lerp(e0[1], e1[1], t)];
    };
    // The arc's height at f: each way's surface carried out level from its edge, weighted by the arc point's
    // distances from the two edges (as heightAt of corners.ts), so it follows both roads' grades.
    const heights = new Map<number, number>();
    const baseZ = (f: number) => {
        if (f <= 0) return za - LIFT_M;
        if (f >= 1) return zb - LIFT_M;
        const known = heights.get(f);
        if (known !== undefined) return known;
        const [wx, wy] = world(J, shifted(ring(0, f), f)), sp = surfaceAt(c.p.line, lpA, ids, wx, wy), sq = surfaceAt(c.q.line, lpB, ids, wx, wy);
        let z = lerp(za, zb, smooth(f));
        if (sp && sq) {
            const dp = Math.max(0, Math.abs(sp.lat) - c.p.line.h), dq = Math.max(0, Math.abs(sq.lat) - c.q.line.h), w = dp + dq > 1e-6 ? dp / (dp + dq) : f;
            z = lerp(sp.edge, sq.edge, w);
        }
        heights.set(f, z - LIFT_M);
        return z - LIFT_M;
    };
    // Each ring vertex tells the look its metres past the edge with a positive `across`: the two ends lie on
    // opposite sides of their lines, and a strip between signed ends would cross the carriageway's middle.
    const at = (f: number, part: Part, mat: number): W => {
        const flags = part.flags ?? 0, sampled = (flags & 3) !== 0;
        if (f === 0) return { ...vertex(cp, sa, c.sp * (sa.h + part.a), part.dz, mat, flags, sa.h + part.a), z: sampled ? part.dz : zOf(cp, sa, c.sp * (sa.h + part.a), part.dz) };
        if (f === 1) return { ...vertex(cq, sb, c.sq * (sb.h + part.a), part.dz, mat, flags, sb.h + part.a), z: sampled ? part.dz : zOf(cq, sb, c.sq * (sb.h + part.a), part.dz) };
        const [px, py] = world(J, shifted(ring(part.a, f), f)), z = sampled ? part.dz : baseZ(f) + part.dz;
        // A band vertex holds the road's height (as vertex of corridor.ts).
        return { i: b.vertex(px, py, sampled ? baseZ(f) : z, corridorColour(mat, code, flags, prio), half_ + part.a, half_, 0), x: px, y: py, z };
    };
    // Strips between columns, each in its outer part's material (the face takes its first vertex's).
    const edgePart: Part = { a: 0, dz: LIFT_M, mat: MAT.PAINT };
    for (let i = 0; i < ARC_STEPS; i++) {
        const f0 = i / ARC_STEPS, f1 = (i + 1) / ARC_STEPS, P = [edgePart, ...partsAt(f0)], Q = [edgePart, ...partsAt(f1)];
        const mid = t0 + sweep * (f0 + f1) / 2, toward = [Math.cos(mid), -Math.sin(mid), 0];
        for (let r = 0; r + 1 < P.length; r++) {
            const mat = Q[r + 1].mat;
            // Rings past the arc's centre collapse to it: no face between two of them.
            if (Math.min(P[r].a, Q[r].a) >= c.r) continue;
            faceToward(b, [at(f0, P[r], mat), at(f0, P[r + 1], mat), at(f1, Q[r + 1], mat), at(f1, Q[r], mat)], toward);
        }
    }
    // The fillet: from the ways' own edge vertices, a → P → b, to the arc, in asphalt.
    if (c.r > 0) {
        // One way's edge from its arc end to the edges' meeting point P: a through way's stations up to its station at P; a
        // side arm's up to its first whole station T, then the through way's vertex at P.
        const along = c.p.through ? c.Pp : c.Pq;
        const P = along === undefined ? undefined : edgeChain(c.p.through ? cp : cq, c.p.through ? lpA : lpB, c.p.through ? c.sp : c.sq, along, along, JMAT.FILLET, code)[0];
        const half = (w: Way, ctx: Ctx, list: Station[], side: 1 | -1, se: Station, along: number | undefined): W[] | undefined => {
            if (w.through) return along === undefined ? undefined : edgeChain(ctx, list, side, se.along, along, JMAT.FILLET, code);
            const T = mouthT(w);
            if (!T || !P) return undefined;
            const beyond = w.step > 0 ? se.along > T.along : se.along < T.along;
            return [...edgeChain(ctx, list, side, se.along, beyond ? T.along : se.along, JMAT.FILLET, code), P];
        };
        const hp = half(c.p, cp, lpA, c.sp, sa, c.Pp), hq = half(c.q, cq, lpB, c.sq, sb, c.Pq);
        if (hp && hq && hp.length && hq.length) {
            const same = Math.hypot(hp[hp.length - 1].x - hq[hq.length - 1].x, hp[hp.length - 1].y - hq[hq.length - 1].y) < 1;
            const chain = [...hp, ...hq.reverse().slice(same ? 1 : 0)];
            const colour = corridorColour(JMAT.FILLET, code, 0, prio), arc: W[] = [chain[0]];
            for (let i = 1; i < ARC_STEPS; i++) {
                const f = i / ARC_STEPS, [px, py] = world(J, shifted(ring(0, f), f)), z = baseZ(f) + LIFT_M;
                arc.push({ i: b.vertex(px, py, z, colour, half_, half_, 0), x: px, y: py, z });
            }
            arc.push(chain[chain.length - 1]);
            zipper(b, chain, arc);
        }
    }
}

function alongOfVertex(line: Line, v: number): number {
    let along = 0;
    for (let k = 0; k < v; k++) along += Math.hypot(line.xy[2 * k + 2] - line.xy[2 * k], line.xy[2 * k + 3] - line.xy[2 * k + 1]) * metresPerUnit(line.xy[2 * k + 1]);
    return along;
}

/** The edge vertices of a planned line on side `side`, from along `from` to `to` (both ends included), in material `mat` on a surface of `code`. */
function edgeChain(ctx: Ctx, list: Station[], side: 1 | -1, from: number, to: number, mat: number, code: number): W[] {
    const lo = Math.min(from, to) - 0.011, hi = Math.max(from, to) + 0.011, out: W[] = [];
    for (const st of list) {
        if (st.along < lo || st.along > hi) continue;
        const a = side * st.h;
        out.push({ ...vertex(ctx, st, a, LIFT_M, mat, 0, a, code), z: st.z + LIFT_M + st.bank * a + ctx.lift });
    }
    return from <= to ? out : out.reverse();
}

/** Triangles between two chains of vertices that run side by side, each from its first to its last, facing up. */
function zipper(b: MeshBuilder, A: W[], B: W[]): void {
    const param = (C: W[]) => {
        const m = metresPerUnit(C[0].y), out = [0];
        for (let k = 1; k < C.length; k++) out.push(out[k - 1] + Math.hypot(C[k].x - C[k - 1].x, C[k].y - C[k - 1].y) * m);
        return out.map(v => v / (out[out.length - 1] || 1));
    };
    const ta = param(A), tb = param(B);
    let i = 0, k = 0;
    while (i + 1 < A.length || k + 1 < B.length) {
        const advanceA = k + 1 >= B.length || (i + 1 < A.length && ta[i + 1] <= tb[k + 1]);
        const tri = advanceA ? [A[i], A[i + 1], B[k]] : [A[i], B[k + 1], B[k]];
        if (advanceA) i++;
        else k++;
        const m = metresPerUnit(tri[0].y), area = (tri[1].x - tri[0].x) * -(tri[2].y - tri[0].y) * m * m - -(tri[1].y - tri[0].y) * (tri[2].x - tri[0].x) * m * m;
        if (Math.abs(area) < 1e-6) continue;
        if (area > 0) b.face(tri[0].i, tri[1].i, tri[2].i);
        else b.face(tri[0].i, tri[2].i, tri[1].i);
    }
}

/** A side arm's mouth: its carriageway from the through road's own edge vertices to its first whole station T. */
function mouthFaces(ctx: Ctx, mouth: LinePlan['mouths'][number], ctxOf: (line: Line) => Ctx | undefined, stations: (line: Line) => JStation[]): void {
    // The mouth is the through road's surface, as its corners' fillets are.
    const T = mouth.T!, chain: W[] = [], code = (mouth.edge[1].p.through ? mouth.edge[1].p : mouth.edge[1].q).line.code;
    // The through road's edge from the mouth's left corner to its right one, along one line or two that meet at the junction.
    for (const side of [1, -1] as const) {
        const c = mouth.edge[side], onP = c.p.through, t = onP ? c.p : c.q, along = onP ? c.Pp : c.Pq, tc = ctxOf(t.line);
        if (along === undefined || !tc) return;
        // The junction's station, by its place: a station's along is metres on the curve, the plan's on the chords.
        const list = stations(t.line), vx = t.line.xy[2 * t.v], vy = t.line.xy[2 * t.v + 1];
        const j = list.reduce((a, st) => (Math.hypot(st.x - vx, st.y - vy) < Math.hypot(a.x - vx, a.y - vy) ? st : a), list[0]).along;
        const part = edgeChain(tc, list, onP ? c.sp : c.sq, side > 0 ? along : j, side > 0 ? j : along, JMAT.FILLET, code);
        if (!part.length) return;
        chain.push(...(chain.length && Math.hypot(chain[chain.length - 1].x - part[0].x, chain[chain.length - 1].y - part[0].y) < 1 ? part.slice(1) : part));
    }
    const road = carriageway(ctx.line, T.h, T.bank), across = road.map(p => ({ ...vertex(ctx, T, p.a, p.dz, JMAT.FILLET, 0, p.a, code), z: 0 }));
    if (chain.length < 2) return;
    zipper(ctx.b, chain, across);
}

/**
 * A line's zebras, lowered kerbs and give-way lines: the faces that its cuts left out, in their own materials, on
 * each station's own cross-section (its half-width, and the crown that its banking flattens), as the sweep's.
 */
function markings(ctx: Ctx, lp: LinePlan, owns: Owns): void {
    const list = lp.stations!, { line } = ctx, road = (st: Station) => carriageway(line, st.h, st.bank), n = road(list[0]).length;
    const own = (s: Station) => owns(s.x, s.y);
    for (const at of lp.zebras) {
        // The stripes run straight across a bend: each vertex carries its metres across the zebra's own straight middle line.
        const k0 = list.findIndex(st => st.along >= at - 1e-3);
        if (k0 < 0) continue;
        const s0 = k0 > 0 && list[k0].along > at + 1e-3 ? mixStation(list[k0 - 1], list[k0], (at - list[k0 - 1].along) / (list[k0].along - list[k0 - 1].along)) : list[k0];
        const nl = Math.hypot(s0.ax, s0.ay), N = [s0.ax / nl, s0.ay / nl], m = metresPerUnit(s0.y);
        const zv = (st: Station, i: number): V => {
            const part = road(st)[i], x = st.x + st.ax * part.a, y = st.y + st.ay * part.a, z = st.z + part.dz + st.bank * part.a + ctx.lift;
            return { i: ctx.b.vertex(x, y, z, corridorColour(JMAT.ZEBRA, line.code, 0, ctx.prio), part.a, st.h, ((x - s0.x) * N[0] + (y - s0.y) * N[1]) * m), x, y };
        };
        for (let k = 0; k + 1 < list.length; k++) {
            const s = list[k], t = list[k + 1], mid = (s.along + t.along) / 2;
            if (Math.abs(mid - at) > ZEBRA_HALF_M || !own(s)) continue;
            for (let i = 0; i + 1 < n; i++) quad(ctx.b, [zv(s, i), zv(s, i + 1), zv(t, i + 1), zv(t, i)], 1);
        }
    }
    for (const { lo, hi, sides } of lp.lowered) {
        for (let k = 0; k + 1 < list.length; k++) {
            const s = list[k], t = list[k + 1], mid = (s.along + t.along) / 2;
            if (mid < lo - RAMP_M || mid > hi + RAMP_M || !own(s)) continue;
            // The kerb's rise at a station: lowered from lo to hi, ramped back past them.
            const rise = (st: Station) => {
                const d = Math.max(lo - st.along, st.along - hi, 0);
                return lerp(LOWERED_M, KERB_RISE_M, smooth(Math.max(0, Math.min(1, d / RAMP_M))));
            };
            for (const side of [1, -1] as const) {
                if (!(sides & (side > 0 ? 1 : 2))) continue;
                const lowered = (st: Station) => sideOf(line, side, t.v, st.h).map(p => ({ ...p, a: side * p.a, dz: p.mat === MAT.KERB ? p.dz - KERB_RISE_M + rise(st) : p.dz }));
                const ps = lowered(s), pt = lowered(t);
                let is: Part = { a: side * s.h, dz: road(s)[0].dz, mat: MAT.PAINT }, it: Part = { a: side * t.h, dz: road(t)[0].dz, mat: MAT.PAINT };
                for (let i = 0; i < ps.length; i++) {
                    const v = (st: Station, part: Part, mat: number) => vertex(ctx, st, part.a, part.dz, mat, part.flags ?? 0);
                    quad(ctx.b, [v(s, is, ps[i].mat), v(s, ps[i], ps[i].mat), v(t, pt[i], ps[i].mat), v(t, it, ps[i].mat)], side);
                    [is, it] = [ps[i], pt[i]];
                }
            }
        }
    }
    for (const { at, dir, reach } of lp.giveWays) {
        const lo = Math.min(at, at + dir * reach), hi = Math.max(at, at + dir * reach);
        // Each vertex carries its metres back from the line; the colour's parameter says which side the approaching lane is
        // on (traffic keeps right: the left of a line that leaves the junction).
        const code = line.code | ((dir > 0 ? 0 : 1) << 4) | ((reach > GIVE_WAY_M ? 1 : 0) << 5);
        const gv = (st: Station, i: number): V => {
            const part = road(st)[i], x = st.x + st.ax * part.a, y = st.y + st.ay * part.a, z = st.z + part.dz + st.bank * part.a + ctx.lift;
            return { i: ctx.b.vertex(x, y, z, corridorColour(JMAT.GIVE_WAY, code, 0, ctx.prio), part.a, st.h, Math.abs(st.along - at)), x, y };
        };
        for (let k = 0; k + 1 < list.length; k++) {
            const s = list[k], t = list[k + 1], mid = (s.along + t.along) / 2;
            if (mid < lo || mid > hi || !own(s)) continue;
            for (let i = 0; i + 1 < n; i++) quad(ctx.b, [gv(s, i), gv(s, i + 1), gv(t, i + 1), gv(t, i)], 1);
        }
    }
}
