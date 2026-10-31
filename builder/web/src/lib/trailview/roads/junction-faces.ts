// The faces of the junction plan (junctions.ts): round each corner its fillet, then the kerb, the sidewalk and the
// band on rings; in each mouth the side arm's carriageway from the through road's edge. All stand on each station's
// own section, so they meet the sweep's faces exactly. The far range keeps every second column of an arc and the
// rows that the sweep keeps there.
import type { Way } from './arms';
import type { Corner } from './corner';
import type { Mouth } from './junctions';
import { lookWord, type PieceBuilder } from './piece';
import { planeAt } from './planes';
import { carriageway, CF, junctionFace, LIFT_M, LOOK, MAT, sideOf, type Part } from './section';
import { SAME_M, stationIndex, stationOn, type Station } from './stations';
import { quad, under, type Run } from './sweep';

/** Columns of an arc, and the far range's step over them. */
const ARC_STEPS = 8, FAR_STEP = 2;
/** A triangle narrower than this in plan, in world units, stays out, unless it stands up as a step's face: a flat one has no pixels of its own, and a plan's rounding could give it dark ones. */
const SLIVER_UNITS = 0.02;
/** Where a side arm's edge ends this near the through road's, in metres, by plan and by height, the two are one point in a face, whose height is the through road's. */
const ONE_POINT_M = 0.05;

/** A row of a ring: a point of a section at each end of the arc, in metres past the carriageway's edge. */
interface Row { from: Part; to: Part }

/** What the faces of a junction are made of. */
interface Surface { word: number; across: number }

const mix = (a: number, b: number, t: number) => a + (b - a) * t;

/** A corner's surface: its through road's, so tone never mixes within a triangle. */
function surfaceOf({ road }: Corner, prio: number): Surface & { code: number } {
    const { mat, half, across } = junctionFace(road)!;
    return { word: lookWord(mat, road.code, 0, prio, half), across, code: road.code };
}

/** The rows of a side in metres past the carriageway's edge, and its core's outer edge there. */
function rowsOf(line: Way['line'], side: 1 | -1, s: Station): { rows: Part[]; edge: number } {
    const { parts, edge } = sideOf(line, side, s.v, s.h);
    return { rows: parts.map(part => ({ ...part, a: part.a - s.h })), edge: edge - s.h };
}

/** Solid rows of a side, past which the band starts. */
const solidOf = (rows: Part[]) => rows.filter(({ flags }) => !(flags & (CF.SAMPLE | CF.MESH)));
/** The rank of a side's solid rows among two: the finer one lies next to the carriageway. */
const rankOf = (rows: Part[]) => 2 * solidOf(rows).length + (solidOf(rows).some(({ mat }) => mat !== LOOK.ASPHALT && mat !== MAT.GROUND) ? 1 : 0);

/**
 * The rows of a ring from one side to the other. Two sides of one shape join row by row. Else the finer side's
 * solid rows narrow to nothing against the carriageway round the arc, as the other's open past them, so a
 * paved shoulder gives way to a verge with no step.
 */
function ringRows(A: Part[], B: Part[]): Row[] {
    if (A.length === B.length && A.every(({ mat }, i) => mat === B[i].mat)) return A.map((from, i) => ({ from, to: B[i] }));
    const band = (rows: Part[]) => rows.slice(solidOf(rows).length), bands = band(A).map((from, i) => ({ from, to: band(B)[i] }));
    const flat = { a: 0, dz: LIFT_M }, outer = (rows: Part[]) => solidOf(rows).at(-1) ?? flat;
    if (rankOf(A) >= rankOf(B)) return [...solidOf(A).map(from => ({ from, to: { ...from, ...flat } })), ...solidOf(B).map(to => ({ from: { ...to, a: outer(A).a, dz: outer(A).dz }, to })), ...bands];
    return [...solidOf(B).map(to => ({ from: { ...to, ...flat }, to })), ...solidOf(A).map(from => ({ from, to: { ...from, a: outer(B).a, dz: outer(B).dz } })), ...bands];
}

/** A vertex in the plan. */
type Point = { i: number; x: number; y: number; z: number };

/** The vertices of a line's carriageway edge on side `side`, from metres along `from` to `to`, both included. */
function edgeChain(b: PieceBuilder, { stations, line, lift }: Run, side: 1 | -1, from: number, to: number, { word, across }: Surface): Point[] {
    const lo = Math.min(from, to) - SAME_M, hi = Math.max(from, to) + SAME_M, out: Point[] = [];
    for (let j = stationIndex(stations, lo); j < stations.length && stations[j].along <= hi; j++) {
        const s = stations[j];
        if (s.along < lo) continue;
        const road = carriageway(line, s.h, s.bank), { a, dz } = road[side > 0 ? 0 : road.length - 1], x = s.x + s.ax * a, y = s.y + s.ay * a, z = s.z + s.bank * a + lift + dz;
        out.push({ i: b.vertex(x, y, z, word, across, 0), x, y, z });
    }
    return from <= to ? out : out.reverse();
}

const onePoint = (p: Point, q: Point, unit: number) => Math.hypot(p.x - q.x, p.y - q.y) * unit < ONE_POINT_M && Math.abs(p.z - q.z) < ONE_POINT_M;

/** A chain on from another, with their meeting as one point where it is one. */
const onTo = (A: Point[], B: Point[], unit: number) => (A.length && B.length && onePoint(A[A.length - 1], B[0], unit) ? [...A.slice(0, -1), ...B] : [...A, ...B]);

/** The metres along a side arm's edge that end its stretch next to the mouth: its arc's end, or the mouth where the arc ends past it. */
const nearMouth = ({ step, mouth }: Way, end: number) => (step > 0 ? Math.min(end, mouth!) : Math.max(end, mouth!));

/** Triangles between two chains of vertices that run side by side, each from its first to its last, facing up; a step's face takes the sense of the rest. */
function zipper(list: number[], A: Point[], B: Point[]): void {
    const shares = (C: Point[]) => {
        const out = [0];
        for (let k = 1; k < C.length; k++) out.push(out[k - 1] + Math.hypot(C[k].x - C[k - 1].x, C[k].y - C[k - 1].y));
        return out.map(v => v / (out[out.length - 1] || 1));
    };
    const ta = shares(A), tb = shares(B), triangles: { p: Point; q: Point; r: Point; area: number; thin: boolean }[] = [];
    for (let i = 0, k = 0; i + 1 < A.length || k + 1 < B.length;) {
        const onA = k + 1 >= B.length || (i + 1 < A.length && ta[i + 1] <= tb[k + 1]), [p, q, r] = onA ? [A[i], A[i + 1], B[k]] : [A[i], B[k + 1], B[k]];
        if (onA) i++;
        else k++;
        // World y runs south.
        const area = (q.x - p.x) * -(r.y - p.y) + (q.y - p.y) * (r.x - p.x), longest = Math.max(Math.hypot(q.x - p.x, q.y - p.y), Math.hypot(r.x - q.x, r.y - q.y), Math.hypot(p.x - r.x, p.y - r.y));
        const thin = Math.abs(area) < SLIVER_UNITS * longest;
        if (!thin || Math.max(p.z, q.z, r.z) - Math.min(p.z, q.z, r.z) >= ONE_POINT_M) triangles.push({ p, q, r, area, thin });
    }
    const sense = triangles.reduce((sum, { area }) => sum + area, 0);
    for (const { p, q, r, area, thin } of triangles) {
        if ((thin ? sense : area) > 0) list.push(p.i, q.i, r.i);
        else list.push(p.i, r.i, q.i);
    }
}

/** The faces of a corner: the fillet between the edges and the arc, then the kerb, the sidewalk and the band round the arc. */
export function cornerFaces(b: PieceBuilder, corner: Corner): void {
    const { p, q, sp, sq, o, r, ends, meeting: { x: jx, y: jy, unit } } = corner, sa = stationOn(p.run.stations, ends[0]), sb = stationOn(q.run.stations, ends[1]);
    if (r <= 0) return;
    const prio = Math.max(p.run.prio, q.run.prio), half = Math.min(sa.h, sb.h), ways = p.run.ways, side = (s: Station, sign: number, a: number) => [(s.x + s.ax * sign * a - jx) * unit, (s.y + s.ay * sign * a - jy) * unit];
    const at0 = side(sa, sp, sa.h), at1 = side(sb, sq, sb.h), r0 = Math.hypot(at0[0] - o[0], at0[1] - o[1]), r1 = Math.hypot(at1[0] - o[0], at1[1] - o[1]);
    const t0 = Math.atan2(at0[1] - o[1], at0[0] - o[0]), t1 = Math.atan2(at1[1] - o[1], at1[0] - o[0]), sweepBy = Math.atan2(Math.sin(t1 - t0), Math.cos(t1 - t0));
    /** The plan of a point `d` past the edge in column k, in metres about the meeting: the ways' own at the arc's ends. */
    const ringAt = (k: number, d: number) => {
        if (k === 0) return side(sa, sp, sa.h + d);
        if (k === ARC_STEPS) return side(sb, sq, sb.h + d);
        const f = k / ARC_STEPS, t = t0 + sweepBy * f, reach = Math.max(0, mix(r0, r1, f) - d);
        return [o[0] + reach * Math.cos(t), o[1] + reach * Math.sin(t)];
    };
    // The arc's height: each way's surface carried out level from its edge, weighted by the distances from the two.
    const za = sa.z + sa.bank * sp * sa.h + p.run.lift, zb = sb.z + sb.bank * sq * sb.h + q.run.lift;
    const edgeZ = Array.from({ length: ARC_STEPS + 1 }, (_, k) => {
        if (k === 0 || k === ARC_STEPS) return k ? zb : za;
        const [ex, ey] = ringAt(k, 0), [x, y] = [jx + ex / unit, jy + ey / unit], [ep, eq] = [planeAt(p, x, y), planeAt(q, x, y)];
        const dp = Math.max(0, Math.abs(ep.left) - sa.h), dq = Math.max(0, Math.abs(eq.left) - sb.h);
        return mix(ep.z, eq.z, dp + dq > 1e-6 ? dp / (dp + dq) : k / ARC_STEPS);
    });
    const A = rowsOf(p.line, sp, sa), B = rowsOf(q.line, sq, sb), rows = ringRows(A.rows, B.rows), surface = surfaceOf(corner, prio), { code } = surface;
    /** A ring's vertices per row, made once per column. */
    const made = new Map<number, number>();
    const vertex = (k: number, row: number) => {
        const name = k * 64 + row, known = made.get(name);
        if (known !== undefined) return known;
        const f = k / ARC_STEPS, { from, to } = rows[row], a = mix(from.a, to.a, f), dz = mix(from.dz, to.dz, f), [mx, my] = ringAt(k, a), x = jx + mx / unit, y = jy + my / unit;
        const tilt = mix(sa.bank * sp, sb.bank * sq, f) * a, plane = edgeZ[k] + tilt, band = (from.flags & (CF.SAMPLE | CF.MESH)) !== 0;
        const edge = half + mix(A.edge, B.edge, f), index = b.vertex(x, y, band ? plane : plane + dz, lookWord(from.mat, code, under(ways, from.flags, x, y), prio, band ? edge : half), half + a, 0);
        made.set(name, index);
        return index;
    };
    /** The vertex of the carriageway's edge in column k, which starts each column's faces. */
    const inner = new Map<number, number>();
    const edgeVertex = (k: number) => {
        const known = inner.get(k);
        if (known !== undefined) return known;
        const [mx, my] = ringAt(k, 0), index = b.vertex(jx + mx / unit, jy + my / unit, edgeZ[k] + LIFT_M, lookWord(rows[0].from.mat, code, 0, prio, half), half, 0);
        inner.set(k, index);
        return index;
    };
    const ring = (list: number[], columns: number, kept: (row: Row) => boolean) => {
        const left = sweepBy < 0;
        for (let k = 0; k < ARC_STEPS; k += columns) {
            let ps = edgeVertex(k), pt = edgeVertex(k + columns), passed = false;
            rows.forEach((row, index) => {
                if (!kept(row) || passed) return;
                const qs = vertex(k, index), qt = vertex(k + columns, index);
                quad(list, ps, qs, qt, pt, left);
                [ps, pt] = [qs, qt];
                // Rows past the arc's centre collapse to it.
                passed = Math.min(mix(row.from.a, row.to.a, k / ARC_STEPS), mix(row.from.a, row.to.a, (k + columns) / ARC_STEPS)) >= r;
            });
        }
    };
    ring(b.near, 1, () => true);
    ring(b.far, FAR_STEP, ({ from }) => from.far);
    fillet(b, corner, surface, t => {
        const [mx, my] = ringAt(t, 0), x = jx + mx / unit, y = jy + my / unit, z = edgeZ[t] + LIFT_M;
        return { x, y, z, i: b.vertex(x, y, z, surface.word, surface.across, 0) };
    });
}

/** The fillet between a corner's arc and the ways' edges from the arc's ends to the point P where they meet. */
function fillet(b: PieceBuilder, { p, q, sp, sq, ends, meet, meeting: { unit } }: Corner, surface: Surface, arcAt: (k: number) => Point): void {
    const through = p.through ? { way: p, side: sp, at: meet[0] } : { way: q, side: sq, at: meet[1] };
    const P = p.through && q.through ? undefined : edgeChain(b, through.way.run, through.side, through.at, through.at, surface)[0];
    /** One way's edge from its arc's end to P: a side arm's as far as its whole carriageway, and on to P across its mouth. */
    const toMeet = (way: Way, side: 1 | -1, end: number, met: number) => {
        if (way.through || !P) return edgeChain(b, way.run, side, end, met, surface);
        return onTo(edgeChain(b, way.run, side, end, nearMouth(way, end), surface), [P], unit);
    };
    const hp = toMeet(p, sp, ends[0], meet[0]), hq = toMeet(q, sq, ends[1], meet[1]);
    if (!hp.length || !hq.length) return;
    const chain = [...hp, ...hq.reverse()], interior = (step: number) => {
        const out: Point[] = [];
        for (let k = step; k < ARC_STEPS; k += step) out.push(arcAt(k));
        return out;
    };
    const near = interior(1), keep = near.filter((_, k) => (k + 1) % FAR_STEP === 0), first = chain[0], last = chain[chain.length - 1];
    zipper(b.near, chain, [first, ...near, last]);
    zipper(b.far, chain, [first, ...keep, last]);
}

/** A side arm's mouth: its carriageway from the arm's first whole station to the through road's own edge, bounded by the arm's own edges, whose heights may stand off the through road's. */
export function mouthFaces(b: PieceBuilder, { way, far, left, right }: Mouth): void {
    const surface = surfaceOf(left, way.run.prio), { unit } = left.meeting;
    const rim = (corner: Corner, side: 1 | -1) => edgeChain(b, way.run, side, far, nearMouth(way, corner.ends[corner.p.through ? 1 : 0]), surface);
    const roadEdge = (corner: Corner, toJunction: boolean) => {
        const onP = corner.p.through, t = onP ? corner.p : corner.q, met = corner.meet[onP ? 0 : 1];
        // Where the arm and the through way are one line, the arm's own edge bounds the mouth.
        return corner.oneLine ? [] : edgeChain(b, t.run, onP ? corner.sp : corner.sq, toJunction ? met : t.along, toJunction ? t.along : met, surface);
    };
    const chain = [rim(left, 1), roadEdge(left, true), roadEdge(right, false), rim(right, -1).reverse()].reduce((A, B) => onTo(A, B, unit));
    if (!chain.length) return;
    const T = stationOn(way.run.stations, far), road = carriageway(way.line, T.h, T.bank);
    // The chain starts and ends on the arm's edges at T, so the faces fan from the section between them, across those edges.
    const between = (parts: Part[]) => parts.slice(1, -1).map(({ a, dz }) => {
        const x = T.x + T.ax * a, y = T.y + T.ay * a, z = T.z + T.bank * a + way.run.lift + dz;
        return { x, y, z, i: b.vertex(x, y, z, surface.word, surface.across, 0) };
    });
    zipper(b.near, chain, between(road));
    zipper(b.far, chain, between(road.filter(part => part.far)));
}
