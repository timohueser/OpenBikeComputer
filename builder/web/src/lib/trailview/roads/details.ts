// PROTOTYPE road system (a study; never merges). Roadside details of a corridor: W-beam guardrails on posts
// where the ground falls away, delineator posts, masonry retaining walls where the ground climbs steeply,
// and the kerb profile of a town street with its lowered kerbs at driveways.
//
// The worker knows no terrain heights, so the CPU builds every candidate rail, post and wall, and the vertex
// stage (details.wgsl) decides from the stamped ground whether it stands: a barrier vertex samples the ground
// beside and along the road, and collapses under the band where the drop or the cut is not there. Ends ramp
// down smoothly, as real terminals do.
//
// A barrier vertex (CF.BARRIER) holds: x, y its plan position; z the road edge's height at its station;
// `across` its metres past the reference edge E (outer edge of the shoulder); `half` its height code (metres
// over the road edge; under -5, metres over the ground at the vertex minus 10; over 5, metres over the wall
// top plus 6); `along` the packed outward angle and grade; the colour's parameter byte the CPU's ramp.
import { metresPerUnit, ROAD, walkOf, type Building, type Line } from '../terrain/bins';
import { LEFT, near, RIGHT, townOf, type Town } from '../terrain/town';
import type { MeshBuilder } from '../structures/mesh';
import { corridorColour } from './corridor';
import { CF, classOf, LIFT_M, MAT, SHOULDER_M, surfaceName, type Part } from './section';
import type { Station } from './stations';

/** Materials of the details, as DM_* of details.wgsl. */
export const DM = { RAIL: 40, RAIL_POST: 41, RAIL_BLOCK: 42, DELI: 44, DELI_BAND: 45, REFLECTOR: 46, WALL: 48, CAP: 49, KERB: 50, EDGING: 51 } as const;

/** What the details read besides the lines: the walks to the houses, the buildings. */
export interface DetailEnv { walks: Line[]; buildings: Building[] }

/** Classes with guardrails where the ground drops, and with delineator posts, outside towns. */
const RAILED = new Set(['trunk', 'trunk_link', 'primary', 'primary_link', 'secondary', 'secondary_link', 'tertiary', 'tertiary_link', 'unclassified']);
const DELINEATED = new Set(['trunk', 'trunk_link', 'primary', 'primary_link', 'secondary', 'secondary_link', 'tertiary', 'tertiary_link', 'unclassified']);
const WALLED = new Set([...RAILED, 'residential', 'living_street']);

/** A guardrail's post spacing; the delineators' spacing on a straight and in a bend. */
const POST_M = 2, DELI_M = 40, DELI_BEND_M = 22;
/** The ramp of a run's end at a junction or a town, and the corner past a junction road's edge that stays open. */
const RAMP_M = 5, MOUTH_M = 5;
/** A town: this many buildings within TOWN_M. */
const TOWN_BUILDINGS = 3, TOWN_M = 40;

/** The W-beam, closed: metres past E and metres over the road edge, the face toward the road first. */
const BEAM: [number, number][] = [[0.42, 0.44], [0.36, 0.49], [0.4, 0.595], [0.36, 0.7], [0.42, 0.75], [0.45, 0.595]];
/** A post: its middle past E, half its depth across and its width along, its top over the road edge; the blockout between it and the beam. */
const POST_A = 0.5, POST_HALF = [0.05, 0.04], POST_TOP = 0.71;
/** A delineator: its middle past E, half its depth and width, its heights over the ground. */
const DELI_A = 0.55, DELI_HALF = [0.05, 0.06], DELI_Z = [0, 0.74, 0.9, 1.0];
/** The wall's face past E, its foot under the road edge; its capstone. */
const WALL_A = 0.08, WALL_FOOT = -0.25, CAP_A = [0.04, 0.42], BACKFILL_M = 3.0;

/** Height codes of `half`: over the ground, over the wall top. */
const OVER_GROUND = -10, OVER_WALL = 6;

/** The kerb of a town street: its face, its bevel, its top; the sidewalk; the edging stone at the sidewalk's outer edge. */
export function kerbParts(a: number, dz: number, walk: number): Part[] {
    return [
        { a, dz: dz + 0.1, mat: DM.KERB },
        { a: a + 0.025, dz: dz + 0.125, mat: DM.KERB },
        { a: a + 0.16, dz: dz + 0.125, mat: DM.KERB },
        { a: a + 0.17, dz: dz + 0.12, mat: DM.KERB },
        { a: a + walk - 0.1, dz: dz + 0.12, mat: MAT.SIDEWALK },
        { a: a + walk - 0.1, dz: dz + 0.135, mat: DM.EDGING },
        { a: a + walk, dz: dz + 0.135, mat: DM.EDGING },
        { a: a + walk + 0.01, dz: dz + 0.07, mat: DM.EDGING },
    ];
}

/** Lowered kerbs: per line and side, the spans along it (metres) where a walk meets the street. */
const lowered = new WeakMap<Line, { side: number; from: number; to: number }[]>();
/** A lowered kerb stands this high: the kerb drops by KERB_DROP_M, over RAMP_KERB_M at each end. */
const KERB_DROP_M = 0.09, RAMP_KERB_M = 1;

/** Metres that a kerb point drops at station s on side `side` (1 left, -1 right), where a walk meets the street. */
export function kerbDrop(line: Line, s: Station, side: number, part: Part): number {
    if (part.mat !== DM.KERB) return 0;
    let f = 0;
    for (const span of lowered.get(line) ?? []) {
        if (span.side !== side) continue;
        f = Math.max(f, Math.min(1, (s.along - span.from + RAMP_KERB_M) / RAMP_KERB_M, (span.to + RAMP_KERB_M - s.along) / RAMP_KERB_M));
    }
    return KERB_DROP_M * Math.max(0, f);
}

/** The details' shared data of a node's lines: the junction mouths, the town, the lowered kerbs. */
export interface DetailPrep { mouths: Map<string, number>; town: Town | null }

/** The prep of a tile's lines, made once: the lines of a tile are the same objects for each of its nodes. */
const preps = new WeakMap<object, DetailPrep>();
/** The ramps and grades of a line, made once. */
const lineData = new WeakMap<Line, { ramp: number[]; mouth: number[]; grade: number[] }>();

export function prepareDetails(lines: Line[], env: DetailEnv | undefined): DetailPrep {
    const known = env && preps.get(env.walks);
    if (known) return known;
    // Per vertex that two lines share: the widest half-width there, keyed by the vertex.
    const seen = new Map<string, { h: number[]; ways: number }>();
    for (const line of lines) {
        for (let v = 0; v < line.z.length; v++) {
            const key = `${line.xy[2 * v]},${line.xy[2 * v + 1]}`, e = seen.get(key) ?? { h: [], ways: 0 };
            e.h.push(line.h);
            e.ways += (v > 0 ? 1 : 0) + (v + 1 < line.z.length ? 1 : 0);
            seen.set(key, e);
        }
    }
    const mouths = new Map<string, number>();
    for (const [key, e] of seen) if (e.h.length > 1 && e.ways > 2) mouths.set(key, Math.max(...e.h));
    if (env) for (const walk of env.walks) lowerFor(lines, walk);
    const prep = { mouths, town: env ? townOf(env.buildings) : null };
    if (env) preps.set(env.walks, prep);
    return prep;
}

/** The span of the street where walk `walk` meets it: its outer end lies at the street's outer edge. */
function lowerFor(lines: Line[], walk: Line): void {
    const ex = walk.xy[2], ey = walk.xy[3], unit = metresPerUnit(ey);
    let best: { line: Line; along: number; side: number; d: number } | undefined;
    for (const line of lines) {
        if (line.kind !== ROAD || !line.sidewalks) continue;
        let along = 0;
        for (let v = 0; v + 1 < line.z.length; v++) {
            const ax = line.xy[2 * v], ay = line.xy[2 * v + 1], dx = line.xy[2 * v + 2] - ax, dy = line.xy[2 * v + 3] - ay, l2 = dx * dx + dy * dy, seg = Math.sqrt(l2) * unit;
            const t = l2 ? Math.min(1, Math.max(0, ((ex - ax) * dx + (ey - ay) * dy) / l2)) : 0;
            const d = Math.hypot(ax + t * dx - ex, ay + t * dy - ey) * unit;
            // World y runs south: the right is where the cross product is positive.
            const side = dx * (ey - ay) - dy * (ex - ax) > 0 ? -1 : 1;
            const walkW = (line.sidewalks[v] ?? 0) & (side > 0 ? LEFT : RIGHT) ? walkOf(line) : 0;
            if (walkW > 0 && Math.abs(d - line.h - walkW) < 1.2 && (!best || Math.abs(d - line.h - walkW) < best.d)) best = { line, along: along + t * seg, side, d: Math.abs(d - line.h - walkW) };
            along += seg;
        }
    }
    if (!best) return;
    const spans = lowered.get(best.line) ?? [];
    spans.push({ side: best.side, from: best.along - walk.h, to: best.along + walk.h });
    lowered.set(best.line, spans);
}


/** The reference edge E of a side: the carriageway's half-width plus its shoulder. */
const edgeOf = (line: Line) => line.h + (SHOULDER_M[surfaceName(line)] ?? 0);

/** Per station of a line, its ramp 0..1 for rails and posts: zero in a junction's mouth and in a town, ramping up over RAMP_M. */
function ramps(line: Line, st: Station[], prep: DetailPrep, towns: boolean): number[] {
    const n = line.z.length, joins: { x: number; y: number; gap: number }[] = [];
    for (let v = 0; v < n; v++) {
        const x = line.xy[2 * v], y = line.xy[2 * v + 1], h = prep.mouths.get(`${x},${y}`);
        if (h !== undefined) joins.push({ x, y, gap: h + MOUTH_M });
    }
    const ok = st.map(s => {
        const unit = metresPerUnit(s.y);
        if (joins.some(j => Math.hypot(j.x - s.x, j.y - s.y) * unit < j.gap)) return false;
        return !(towns && prep.town && townish(prep.town, s.x, s.y, unit));
    });
    // Metres along to the nearest station that is not ok, both ways.
    const dist = st.map(() => Infinity);
    for (let j = 0, last = -Infinity; j < st.length; j++) {
        if (!ok[j]) last = st[j].along;
        dist[j] = st[j].along - last;
    }
    for (let j = st.length - 1, last = Infinity; j >= 0; j--) {
        if (!ok[j]) last = st[j].along;
        dist[j] = Math.min(dist[j], last - st[j].along);
    }
    return dist.map(d => Math.max(0, Math.min(1, d / RAMP_M)));
}

/** Whether TOWN_BUILDINGS buildings stand within TOWN_M of a point. */
function townish(town: Town, x: number, y: number, unit: number): boolean {
    let count = 0;
    near(town, x, y, x, y, TOWN_M / unit, () => count++);
    return count >= TOWN_BUILDINGS;
}

/** The packed outward angle and grade of `along`, as details.wgsl unpacks them. */
function packed(ox: number, oy: number, grade: number): number {
    const angle = Math.round(((Math.atan2(oy, ox) + Math.PI) / (2 * Math.PI)) * 4096) & 4095;
    return angle * 4096 + Math.max(0, Math.min(4095, Math.round(grade * 4096 + 2048)));
}

/** A point of a detail on the CPU: its vertex, and its place in metres east, north, up of an origin. */
interface P { i: number; e: number; n: number; u: number }

/** One side of one line: the station data a detail vertex needs. */
interface Side { b: MeshBuilder; side: 1 | -1; E: number; made: Map<string, { i: number; x: number; y: number }> }

/** A detail vertex at a station (or a point between stations) `a` metres past E and `al` metres along, at height code `dz`. */
function dv(sd: Side, s: Station, ramp: number, grade: number, a: number, al: number, dz: number, mat: number, origin: Station): P {
    const { b, side, E } = sd, key = s.j === undefined || al ? undefined : `${s.j}|${side}|${a}|${dz}|${mat}`;
    let v = key === undefined ? undefined : sd.made.get(key);
    if (!v) {
        const ox = side * s.ax, oy = side * s.ay, fx = -s.ay, fy = s.ax;
        const x = s.x + ox * (E + a) + fx * al, y = s.y + oy * (E + a) + fy * al, edge = s.z + LIFT_M + s.bank * side * E;
        v = { i: b.vertex(x, y, edge, corridorColour(mat, Math.round(255 * ramp), CF.BARRIER), a, dz, packed(ox, oy, side * grade)), x, y };
        if (key !== undefined) sd.made.set(key, v);
    }
    const { i, x, y } = v;
    const mo = metresPerUnit(origin.y);
    // A rough height for the winding: the code's metres over the edge, or over the ground (taken as the edge).
    const up = dz < -5 ? dz - OVER_GROUND - 0.3 : dz > 5 ? dz - OVER_WALL + 1.5 : dz;
    return { i, e: (x - origin.x) * mo, n: -(y - origin.y) * mo, u: up };
}

/** A quad of four points, turned to face `out` (east, north, up). */
function face(b: MeshBuilder, q: [P, P, P, P], out: [number, number, number]): void {
    const [p0, p1, p2, p3] = q, u = [p2.e - p0.e, p2.n - p0.n, p2.u - p0.u], w = [p3.e - p1.e, p3.n - p1.n, p3.u - p1.u];
    const n = [u[1] * w[2] - u[2] * w[1], u[2] * w[0] - u[0] * w[2], u[0] * w[1] - u[1] * w[0]];
    if (n[0] * out[0] + n[1] * out[1] + n[2] * out[2] >= 0) b.face(p0.i, p1.i, p2.i, p3.i);
    else b.face(p0.i, p3.i, p2.i, p1.i);
}

/** The direction east, north, up of `a` across (outward positive) and `up` at station s on a side. */
function outOf(sd: Side, s: Station, a: number, up: number): [number, number, number] {
    const l = Math.hypot(s.ax, s.ay) || 1;
    return [(sd.side * s.ax * a) / l, (-sd.side * s.ay * a) / l, up];
}

/** The station a share u of the way from s to t. */
function lerp(s: Station, t: Station, u: number): Station {
    const f = (p: number, q: number) => p + (q - p) * u;
    return { ...s, x: f(s.x, t.x), y: f(s.y, t.y), z: f(s.z, t.z), ax: f(s.ax, t.ax), ay: f(s.ay, t.ay), along: f(s.along, t.along), bank: f(s.bank, t.bank), j: undefined };
}

/**
 * A box between stations' heights: corners `a0..a1` past E, `al` ± half along, heights `z0..z1` (codes); its
 * four sides and its top, each in material `mat`, or `mats` from the bottom up.
 */
function box(sd: Side, s: Station, ramp: number, grade: number, a: [number, number], half: number, z: [number, number], mat: number, top = true): void {
    const v = (aa: number, al: number, zz: number) => dv(sd, s, ramp, grade, aa, al, zz, mat, s);
    const lo = [v(a[0], -half, z[0]), v(a[1], -half, z[0]), v(a[1], half, z[0]), v(a[0], half, z[0])];
    const hi = [v(a[0], -half, z[1]), v(a[1], -half, z[1]), v(a[1], half, z[1]), v(a[0], half, z[1])];
    const fx = -s.ay, fy = s.ax, l = Math.hypot(fx, fy) || 1, along: [number, number, number] = [fx / l, -fy / l, 0];
    const outs: [number, number, number][] = [[-along[0], -along[1], 0], outOf(sd, s, 1, 0), along, outOf(sd, s, -1, 0)];
    for (let k = 0; k < 4; k++) face(sd.b, [lo[k], lo[(k + 1) % 4], hi[(k + 1) % 4], hi[k]], outs[k]);
    if (top) face(sd.b, [hi[0], hi[1], hi[2], hi[3]], [0, 0, 1]);
}

/**
 * The details between stations j and j+1 of a line (the node owns station j): per side without a sidewalk,
 * the guardrail's beam and posts, the delineators and the retaining wall.
 */
export function roadside(b: MeshBuilder, line: Line, st: Station[], j: number, prep: DetailPrep, cache: Map<Line, Side['made']>): void {
    if (line.kind !== ROAD) return;
    const cls = classOf(line), railed = RAILED.has(cls), walled = WALLED.has(cls), delineated = DELINEATED.has(cls);
    if (!railed && !walled) return;
    let c = lineData.get(line);
    if (!c) {
        const grade = st.map((_, k) => {
            const p = st[Math.max(0, k - 1)], q = st[Math.min(st.length - 1, k + 1)];
            return q.along > p.along ? (q.z - p.z) / (q.along - p.along) : 0;
        });
        lineData.set(line, (c = { ramp: ramps(line, st, prep, true), mouth: ramps(line, st, prep, false), grade }));
    }
    const made = cache.get(line) ?? cache.set(line, new Map()).get(line)!;
    const s = st[j], t = st[j + 1];
    for (const side of [1, -1] as const) {
        if ((line.sidewalks?.[s.v] ?? 0) & (side > 0 ? LEFT : RIGHT)) continue;
        const sd: Side = { b, side, E: edgeOf(line), made };
        const rs = c.ramp[j], rt = c.ramp[j + 1], gs = c.grade[j], gt = c.grade[j + 1];
        if (railed && (rs > 0 || rt > 0)) {
            beam(sd, s, t, rs, rt, gs, gt);
            for (let p = Math.ceil(s.along / POST_M) * POST_M; p < t.along; p += POST_M) {
                const u = (p - s.along) / Math.max(t.along - s.along, 1e-6), at = lerp(s, t, u), r = rs + (rt - rs) * u, g = gs + (gt - gs) * u;
                box(sd, at, r, g, [POST_A - POST_HALF[0], POST_A + POST_HALF[0]], POST_HALF[1], [OVER_GROUND - 0.3, POST_TOP], DM.RAIL_POST);
            }
        }
        if (delineated) {
            const spacing = bendy(st, j) ? DELI_BEND_M : DELI_M;
            for (let p = Math.ceil(s.along / spacing) * spacing; p < t.along; p += spacing) {
                const u = (p - s.along) / Math.max(t.along - s.along, 1e-6), at = lerp(s, t, u), r = rs + (rt - rs) * u, g = gs + (gt - gs) * u;
                delineator(sd, at, r, g);
            }
        }
        if (walled && (c.mouth[j] > 0 || c.mouth[j + 1] > 0)) wall(sd, s, t, c.mouth[j], c.mouth[j + 1], gs, gt);
    }
}

/** Whether a line turns more than 0.35 rad within 10 stations of station j. */
function bendy(st: Station[], j: number): boolean {
    const p = st[Math.max(0, j - 6)], q = st[Math.min(st.length - 1, j + 6)];
    const cos = (p.ax * q.ax + p.ay * q.ay) / (Math.hypot(p.ax, p.ay) * Math.hypot(q.ax, q.ay) || 1);
    return cos < Math.cos(0.35);
}

/** The W-beam between stations s and t: the closed profile swept. */
function beam(sd: Side, s: Station, t: Station, rs: number, rt: number, gs: number, gt: number): void {
    const ps = BEAM.map(([a, dz]) => dv(sd, s, rs, gs, a, 0, dz, DM.RAIL, s)), pt = BEAM.map(([a, dz]) => dv(sd, t, rt, gt, a, 0, dz, DM.RAIL, s));
    const n = BEAM.length;
    // The profile runs clockwise in (across, up) seen with the road on the left: its outward normal is (dz, -da) turned.
    let area = 0;
    for (let k = 0; k < n; k++) area += BEAM[k][0] * BEAM[(k + 1) % n][1] - BEAM[(k + 1) % n][0] * BEAM[k][1];
    for (let k = 0; k < n; k++) {
        const [a0, z0] = BEAM[k], [a1, z1] = BEAM[(k + 1) % n], sign = area > 0 ? 1 : -1;
        face(sd.b, [ps[k], ps[(k + 1) % n], pt[(k + 1) % n], pt[k]], outOf(sd, s, sign * (z1 - z0), -sign * (a1 - a0)));
    }
}

/** A delineator post: white, a black band with an amber reflector toward the road, a white cap. */
function delineator(sd: Side, s: Station, r: number, g: number): void {
    const a: [number, number] = [DELI_A - DELI_HALF[0], DELI_A + DELI_HALF[0]], h = DELI_HALF[1], z = DELI_Z.map(v => v + OVER_GROUND);
    box(sd, s, r, g, a, h, [z[0] - 0.3, z[1]], DM.DELI, false);
    box(sd, s, r, g, a, h, [z[1], z[2]], DM.DELI_BAND, false);
    box(sd, s, r, g, a, h, [z[2], z[3]], DM.DELI);
    // The reflector, a hair proud of the face toward the road.
    const v = (al: number, zz: number) => dv(sd, s, r, g, a[0] - 0.004, al, zz, DM.REFLECTOR, s);
    face(sd.b, [v(-0.03, z[1] + 0.03), v(0.03, z[1] + 0.03), v(0.03, z[2] - 0.03), v(-0.03, z[2] - 0.03)], outOf(sd, s, -1, 0));
}

/** The retaining wall between stations s and t: a vertical masonry face, its capstone, and the ground it holds back to BACKFILL_M. */
function wall(sd: Side, s: Station, t: Station, rs: number, rt: number, gs: number, gt: number): void {
    const prof: [number, number, number][] = [[WALL_A, WALL_FOOT, DM.WALL], [WALL_A, OVER_WALL - 0.08, DM.WALL]];
    const cap: [number, number, number][] = [[CAP_A[0], OVER_WALL - 0.08, DM.CAP], [CAP_A[0], OVER_WALL + 0.04, DM.CAP], [CAP_A[1], OVER_WALL + 0.04, DM.CAP], [CAP_A[1], OVER_WALL - 0.03, DM.CAP]];
    // The ground behind the wall, from its capstone back to the stamped ground.
    const fill: [number, number, number][] = [[CAP_A[1], OVER_WALL - 0.03, MAT.GROUND], [BACKFILL_M, OVER_GROUND - 0.05, MAT.GROUND]];
    for (const pr of [prof, cap, fill]) {
        const ps = pr.map(([a, dz, mat]) => dv(sd, s, rs, gs, a, 0, dz, mat, s)), pt = pr.map(([a, dz, mat]) => dv(sd, t, rt, gt, a, 0, dz, mat, s));
        for (let k = 0; k + 1 < pr.length; k++) {
            const [a0, z0] = pr[k], [a1, z1] = pr[k + 1];
            // Each face of the wall faces the road or up: (-dz, da) in (across, up).
            face(sd.b, [ps[k], ps[k + 1], pt[k + 1], pt[k]], outOf(sd, s, -(z1 - z0), a1 - a0));
        }
    }
}
