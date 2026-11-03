// The faces of the junction plan (junctions.ts): round each corner its fillet, then the kerb, the sidewalk and the
// band on rings; in each mouth the side arm's carriageway from the through road's edge; at each seam the guest's fill
// from its stop to the host's outer edge. All stand on each station's own section, so they meet the sweep's faces exactly. The far range keeps every second column of an arc and the
// rows that the sweep keeps there.
import { soil } from '../terrain/bins';
import { EDGE_BEHIND_M } from '../terrain/junction-plan';
import { footOn, outward, WINDOW_M, type Way } from './arms';
import type { Corner } from './corner';
import type { Mouth, Seam } from './junctions';
import { lookWord, type PieceBuilder } from './piece';
import { carriageway, CF, drawnCode, junctionFace, LIFT_M, lookOf, LOOK, MAT, sideOf, type Part } from './section';
import { sideStop } from './shore';
import { SAME_M, stationIndex, stationOn, type Station } from './stations';
import { quad, under, type Run } from './sweep';

/** Columns of an arc, and the far range's step over them. */
const ARC_STEPS = 8, FAR_STEP = 2;
/** A triangle narrower than this in plan, in world units, stays out: a flat one has no pixels of its own, and a plan's rounding could give it dark ones. */
const SLIVER_UNITS = 0.02;
/** The step that tells a column's direction from its plan. */
const COLUMN_M = 0.01;
/** Where two chains of a face end this near in plan, in metres, they are one point: a fill has no upright step. */
const ONE_POINT_M = 0.05;

/** The height of a way's surface at a point near it, without its lift over the core: its banked plane, held level past its edges. */
function planeAt(way: Way, x: number, y: number): { z: number; left: number } {
    const { s, t, u, left } = footOn(way, x, y), mix = (a: number, b: number) => a + (b - a) * u, h = mix(s.h, t.h);
    return { z: mix(s.z, t.z) + mix(s.bank, t.bank) * Math.max(-h, Math.min(h, left)) + way.run.lift, left };
}

/** A row of a ring: a point of a section at each end of the arc, in metres past the carriageway's edge. */
interface Row { from: Part; to: Part }

/** What the faces of a junction are made of: a look, and a point's metres across it, the same at every point or by `at`. */
interface Surface { word: number; across: number; at?: (x: number, y: number) => [number, number] }

/** A vertex of a surface. */
const surfaceVertex = (b: PieceBuilder, x: number, y: number, z: number, { word, across, at }: Surface) => {
    const [a, along] = at ? at(x, y) : [across, 0];
    return b.vertex(x, y, z, word, a, along);
};

/**
 * A guest's own look, as its carriageway has it, with each point's metres across the guest's line and out along it from
 * the junction: a fill in it reads as the way run on to its seam. Its count starts at the junction, as a tile that cuts
 * the line elsewhere counts the run's own from other metres, and the faces of a junction are the same in each tile; it
 * runs out, as a vertex holds no count below 0.
 */
function ownLook(way: Way, h: number): Surface {
    const { run, step } = way;
    return {
        word: lookWord(lookOf(run.line), drawnCode(run.line), 0, run.prio, h),
        across: 0,
        at: (x, y) => {
            const { s, t, u, left } = footOn(way, x, y);
            return [left * step, (s.along + u * (t.along - s.along) - way.along) * step];
        },
    };
}

const mix = (a: number, b: number, t: number) => a + (b - a) * t;

/** A corner's surface: its through road's, so tone never mixes within a triangle. */
function surfaceOf({ road }: Corner, prio: number): Surface & { code: number } {
    const { mat, half, across } = junctionFace(road)!;
    const code = drawnCode(road);
    return { word: lookWord(mat, code, 0, prio, half), across, code };
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

/** A point in the plan, and its height. */
type At = { x: number; y: number; z: number };
/** A vertex in the plan. */
type Point = At & { i: number };

/** The points of a line's carriageway edge on side `side`, from metres along `from` to `to`, both included. */
function edgePoints({ stations, line, lift }: Run, side: 1 | -1, from: number, to: number): At[] {
    const lo = Math.min(from, to) - SAME_M, hi = Math.max(from, to) + SAME_M, out: At[] = [];
    for (let j = stationIndex(stations, lo); j < stations.length && stations[j].along <= hi; j++) {
        const s = stations[j];
        if (s.along < lo) continue;
        const road = carriageway(line, s.h, s.bank), { a, dz } = road[side > 0 ? 0 : road.length - 1];
        out.push({ x: s.x + s.ax * a, y: s.y + s.ay * a, z: s.z + s.bank * a + lift + dz });
    }
    return from <= to ? out : out.reverse();
}

/** The vertices of a line's carriageway edge on side `side`, from metres along `from` to `to`, both included. */
const edgeChain = (b: PieceBuilder, run: Run, side: 1 | -1, from: number, to: number, surface: Surface): Point[] => edgePoints(run, side, from, to).map(p => ({ ...p, i: surfaceVertex(b, p.x, p.y, p.z, surface) }));

const onePoint = (p: At, q: At, unit: number) => Math.hypot(p.x - q.x, p.y - q.y) * unit < ONE_POINT_M;

/** A chain on from another, with their meeting as one point where it is one. */
const onTo = <P extends At>(A: P[], B: P[], unit: number) => (A.length && B.length && onePoint(A[A.length - 1], B[0], unit) ? [...A.slice(0, -1), ...B] : [...A, ...B]);

/**
 * Triangles between two chains of vertices that run side by side, each from its first to its last, facing up: paired by
 * their shares of each chain's length, or by `keys` that rise along each. A face thinner in plan than SLIVER_UNITS stays
 * out: the plan gives both chains their heights from one surface, so such a face closes no step, and an upright one
 * would stand as a torn face.
 */
function zipper(list: number[], A: Point[], B: Point[], keys?: [number[], number[]]): void {
    const shares = (C: Point[]) => {
        const out = [0];
        for (let k = 1; k < C.length; k++) out.push(out[k - 1] + Math.hypot(C[k].x - C[k - 1].x, C[k].y - C[k - 1].y));
        return out.map(v => v / (out[out.length - 1] || 1));
    };
    const [ta, tb] = keys ?? [shares(A), shares(B)];
    for (let i = 0, k = 0; i + 1 < A.length || k + 1 < B.length;) {
        const onA = k + 1 >= B.length || (i + 1 < A.length && ta[i + 1] <= tb[k + 1]), [p, q, r] = onA ? [A[i], A[i + 1], B[k]] : [A[i], B[k + 1], B[k]];
        if (onA) i++;
        else k++;
        // World y runs south.
        const area = (q.x - p.x) * -(r.y - p.y) + (q.y - p.y) * (r.x - p.x), longest = Math.max(Math.hypot(q.x - p.x, q.y - p.y), Math.hypot(r.x - q.x, r.y - q.y), Math.hypot(p.x - r.x, p.y - r.y));
        if (Math.abs(area) < SLIVER_UNITS * longest) continue;
        if (area > 0) list.push(p.i, q.i, r.i);
        else list.push(p.i, r.i, q.i);
    }
}

/** The faces of a corner: the fillet between the edges and the arc, then the kerb, the sidewalk and the band round the arc, each column up to the water's edge. */
export function cornerFaces(b: PieceBuilder, corner: Corner): void {
    const { p, q, sp, sq, o, r, ends, meeting: { x: jx, y: jy, unit } } = corner, sa = stationOn(p.run.stations, ends[0]), sb = stationOn(q.run.stations, ends[1]);
    if (r <= 0) return;
    const prio = Math.max(p.run.prio, q.run.prio), half = Math.min(sa.h, sb.h), ways = p.run.ways, side = (s: Station, sign: number, a: number) => [(s.x + s.ax * sign * a - jx) * unit, (s.y + s.ay * sign * a - jy) * unit];
    const at0 = side(sa, sp, sa.h), at1 = side(sb, sq, sb.h), r0 = Math.hypot(at0[0] - o[0], at0[1] - o[1]), r1 = Math.hypot(at1[0] - o[0], at1[1] - o[1]);
    const t0 = Math.atan2(at0[1] - o[1], at0[0] - o[0]), t1 = Math.atan2(at1[1] - o[1], at1[0] - o[0]), sweepBy = Math.atan2(Math.sin(t1 - t0), Math.cos(t1 - t0));
    /** The plan of a point `d` past the edge in column k, in metres about the meeting: the ways' own at the arc's ends. */
    // An end's column runs along its way's section, the arc's radius there: past the centre it would cross the columns
    // beside it, so it stops at the centre as they do, and the ways' own bands cover the ground behind.
    const ringAt = (k: number, d: number) => {
        if (k === 0) return side(sa, sp, sa.h + Math.min(d, r0));
        if (k === ARC_STEPS) return side(sb, sq, sb.h + Math.min(d, r1));
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
    /** The carriageway edge's own height over that plane in column k, as the sweep draws it at the arc's ends: a paved way's bed raises it. */
    const rim = (line: Way['line'], st: Station, side: 1 | -1) => carriageway(line, st.h, st.bank).at(side > 0 ? 0 : -1)!.dz, [ra, rb] = [rim(p.line, sa, sp), rim(q.line, sb, sq)];
    const edgeDz = (k: number) => mix(ra, rb, k / ARC_STEPS);
    const A = rowsOf(p.line, sp, sa), B = rowsOf(q.line, sq, sb), rows = ringRows(A.rows, B.rows), surface = surfaceOf(corner, prio), { code } = surface;
    const { shore } = p.run;
    /** Where water stops a column, in metres past the edge. */
    const stops = Array.from({ length: ARC_STEPS + 1 }, (_, k) => {
        if (!shore) return Infinity;
        const [ex, ey] = ringAt(k, 0), [fx, fy] = ringAt(k, COLUMN_M);
        return sideStop(shore, jx + ex / unit, jy + ey / unit, (fx - ex) / COLUMN_M / unit, (fy - ey) / COLUMN_M / unit, () => mix(A.edge, B.edge, k / ARC_STEPS));
    });
    /** A ring's vertices per row, made once per column; a row's points that reach the arc's centre are one vertex there. */
    const made = new Map<number, number>();
    const vertex = (k: number, row: number) => {
        const f = k / ARC_STEPS, { from, to } = rows[row], a = Math.min(mix(from.a, to.a, f), stops[k]), centre = a >= (k === 0 ? r0 : k === ARC_STEPS ? r1 : mix(r0, r1, f));
        const name = centre ? -1 - row : k * 64 + row, known = made.get(name);
        if (known !== undefined) return known;
        const dz = mix(from.dz, to.dz, f), [mx, my] = ringAt(k, a), x = jx + mx / unit, y = jy + my / unit;
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
        const [mx, my] = ringAt(k, 0), index = b.vertex(jx + mx / unit, jy + my / unit, edgeZ[k] + edgeDz(k), lookWord(rows[0].from.mat, code, 0, prio, half), half, 0);
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
        const [mx, my] = ringAt(t, 0), x = jx + mx / unit, y = jy + my / unit, z = edgeZ[t] + edgeDz(t);
        return { x, y, z, i: b.vertex(x, y, z, surface.word, surface.across, 0) };
    });
}

/** The fillet between a corner's arc and the ways' edges from the arc's ends to the point P where they meet. */
function fillet(b: PieceBuilder, { p, q, sp, sq, ends, meet, meeting: { unit } }: Corner, surface: Surface, arcAt: (k: number) => Point): void {
    const through = p.through ? { way: p, side: sp, at: meet[0] } : { way: q, side: sq, at: meet[1] };
    const P = p.through && q.through ? undefined : edgeChain(b, through.way.run, through.side, through.at, through.at, surface)[0];
    /** One way's edge from its arc's end to P: a side arm's on its stations, as its mouth's outer lane runs, to P on the through road's edge. */
    const toMeet = (way: Way, side: 1 | -1, end: number, met: number) => {
        if (way.through || !P) return edgeChain(b, way.run, side, end, met, surface);
        return onTo(edgeChain(b, way.run, side, end, met, surface), [P], unit);
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

/** Where a lane ends: the share t along its segment, and the metres along the through ways' edge as segment k and share u. */
type Cross = { t: number; k: number; u: number };

/** A share of a segment within which its end lies on another. */
const TOUCH = 1e-9;

/** A point of a polyline: segment k at share u. */
const pointOn = (line: At[], k: number, u: number): At => ({ x: mix(line[k].x, line[k + 1].x, u), y: mix(line[k].y, line[k + 1].y, u), z: mix(line[k].z, line[k + 1].z, u) });

/** Where segment p-q first crosses a polyline. */
function crossOf(line: At[], p: At, q: At): Cross | undefined {
    let best: Cross | undefined;
    for (let k = 0; k + 1 < line.length; k++) {
        const a = line[k], c = line[k + 1], rx = q.x - p.x, ry = q.y - p.y, sx = c.x - a.x, sy = c.y - a.y, cross = rx * sy - ry * sx;
        if (Math.abs(cross) < 1e-18) continue;
        const t = ((a.x - p.x) * sy - (a.y - p.y) * sx) / cross, u = ((a.x - p.x) * ry - (a.y - p.y) * rx) / cross;
        // A segment that ends on the line meets it there.
        if (t >= -TOUCH && t <= 1 + TOUCH && u >= -TOUCH && u <= 1 + TOUCH && !(best && t >= best.t)) best = { t: Math.max(0, Math.min(1, t)), k, u: Math.max(0, Math.min(1, u)) };
    }
    return best;
}

/**
 * The through ways' carriageway edge on a mouth's side, from far along the left corner's through way to far along the
 * right one's. A way that ends at the junction runs on straight behind it, as its end stands on a joint, and the two
 * meet where they cross nearest the junction: so the edge is the outline of the two carriageways, wherever a mouth's
 * lanes reach it.
 */
function throughEdge(left: Corner, right: Corner): At[] {
    const { x, y, unit } = left.meeting;
    const arm = (corner: Corner, behind = true) => {
        const onP = corner.p.through, t = onP ? corner.p : corner.q, points = edgePoints(t.run, onP ? corner.sp : corner.sq, t.along, t.along + t.step * WINDOW_M);
        const j = stationIndex(t.run.stations, t.along);
        if (!behind || points.length < 2 || (j > 0 && j + 1 < t.run.stations.length)) return points;
        const [p, q] = points, l = Math.hypot(p.x - q.x, p.y - q.y) || 1;
        return [{ x: p.x + ((p.x - q.x) / l) * (EDGE_BEHIND_M / unit), y: p.y + ((p.y - q.y) / l) * (EDGE_BEHIND_M / unit), z: p.z }, ...points];
    };
    const A = arm(left).reverse(), B = arm(right);
    let best: { i: number; met: Cross; away: number } | undefined;
    for (let i = 0; i + 1 < A.length; i++) {
        for (let k = 0; k + 1 < B.length; k++) {
            const met = crossOf([B[k], B[k + 1]], A[i], A[i + 1]);
            if (!met) continue;
            const away = Math.hypot(mix(A[i].x, A[i + 1].x, met.t) - x, mix(A[i].y, A[i + 1].y, met.t) - y);
            if (!best || away < best.away) best = { i, met: { ...met, k }, away };
        }
    }
    if (!best) return onTo(arm(left, false).reverse(), arm(right, false), unit);
    const { i, met } = best;
    return [...A.slice(0, i + 1), pointOn(A, i, met.t), ...B.slice(met.k + 1)];
}

/**
 * A side arm's mouth: its carriageway from its stop to the through road's edge. Each point of the arm's section runs as
 * a lane through the arm's stations toward the junction, on straight past it, to where it crosses the through ways'
 * edge on the mouth's side. Two lanes and that edge between their crossings bound a strip: quads between the arm's
 * sections, then a fan to the edge. So a face lies between two sections and two lanes however long, thin or acute the
 * mouth, and the mouth meets the through road along its own edge: none folds or flaps. A faced arm's mouth takes the
 * through road's junction look; a way of soil, which has none, its own.
 */
export function mouthFaces(b: PieceBuilder, { way, far, left, right }: Mouth): void {
    const { run, step } = way, { stations, line, lift } = run, { unit } = left.meeting, T = stationOn(stations, far);
    const surface = soil(line) ? ownLook(way, T.h) : surfaceOf(left, run.prio);
    const edge = throughEdge(left, right);
    if (edge.length < 2) return;
    // The rows: the arm's sections from its stop to its junction, as the stop's, so each lane keeps its point.
    const rows: Station[] = [], beyond: Station[] = [], jT = stationIndex(stations, far), last = stations.indexOf(outward(way).at(-1)!);
    for (let j = jT, j0 = stationIndex(stations, way.along); j >= 0 && j < stations.length; j -= step) {
        rows.push(stations[j]);
        if (j === j0) break;
    }
    for (let j = last; (j - jT) * step > 0; j -= step) beyond.push(stations[j]);
    if (rows.length < 2) return;
    const at = (s: Station, { a, dz }: Part): At => ({ x: s.x + s.ax * a, y: s.y + s.ay * a, z: s.z + s.bank * a + lift + dz });
    /**
     * A lane: its points from the stop, each with its key (rows from the stop), and where it ends on the edge. It is
     * read in from where the plan reads the arm last: where it meets the edge at the stop, the stop's point ends its
     * lane, and before it, that point lies inside the through road.
     */
    const lane = (index: number) => {
        const point = (s: Station) => at(s, carriageway(line, s.h, s.bank, T.ruts)[index]), points = rows.map(point), path = [...beyond.map(point), ...points];
        const [p, q] = points.slice(-2), l = Math.hypot(q.x - p.x, q.y - p.y) || 1, on = { x: q.x + ((q.x - p.x) / l) * (WINDOW_M / unit), y: q.y + ((q.y - p.y) / l) * (WINDOW_M / unit), z: q.z };
        for (let g = 0; g < path.length; g++) {
            const r = path[g + 1] ?? on, met = crossOf(edge, path[g], r), length = Math.hypot(r.x - path[g].x, r.y - path[g].y);
            if (!met) continue;
            const k = g - beyond.length, e = met.k + met.u, before = (1 - met.t) * length * unit;
            if (k < 0 || (k === 0 && met.t * length * unit < ONE_POINT_M)) return { points: points.slice(0, 1), keys: [0], e, inside: k < -1 || (k === -1 && before > ONE_POINT_M) };
            const end = pointOn(edge, met.k, met.u);
            // The lane runs straight in height from the stop's section to the through road's edge.
            const kept = [...points.slice(0, k + 1), end], lengths = kept.map((_, n) => (n ? Math.hypot(kept[n].x - kept[n - 1].x, kept[n].y - kept[n - 1].y) : 0));
            for (let n = 1; n < lengths.length; n++) lengths[n] += lengths[n - 1];
            return { points: kept.map((point, n) => ({ ...point, z: mix(kept[0].z, end.z, lengths[n] / (lengths.at(-1) || 1)) })), keys: [...points.slice(0, k + 1).map((_, n) => n), k + met.t], e, inside: false };
        }
        return undefined;
    };
    const parts = carriageway(line, T.h, T.bank, T.ruts), read = parts.map((_, index) => lane(index));
    if (read.some(l => !l)) return;
    // A lane whose stop lies inside the through road starts where the stop's section crosses the edge, toward the
    // nearest lane outside; between two such lanes lies nothing of the mouth.
    const out = read.flatMap((l, i) => (l!.inside ? [] : [i])), stop = read.map(l => l!.points[0]);
    if (!out.length) return;
    const lanes = read.map((l, i) => {
        if (!l!.inside) return l;
        const j = out.reduce((best, o) => (Math.abs(o - i) < Math.abs(best - i) ? o : best)), toward = Math.sign(j - i);
        for (let n = i; n !== j; n += toward) {
            const met = crossOf(edge, stop[n], stop[n + toward]);
            if (met) return { points: [pointOn(edge, met.k, met.u)], keys: [0], e: met.k + met.u, inside: true };
        }
        return { ...read[j]!, points: read[j]!.points.slice(0, 1), keys: [0], inside: true };
    });
    const vertex = (p: At): Point => ({ ...p, i: surfaceVertex(b, p.x, p.y, p.z, surface) }), edgeVertices = new Map<number, Point>();
    const made = lanes.map(l => ({ ...l!, points: l!.points.map(vertex) }));
    /** The strip between two lanes: the shorter one runs on along the edge to the longer one's end. */
    const strip = (list: number[], A: (typeof made)[number], B: (typeof made)[number]) => {
        if (A.inside && B.inside) return;
        const [short, long] = A.keys.at(-1)! <= B.keys.at(-1)! ? [A, B] : [B, A], from = short.e, to = long.e, ks = short.keys.at(-1)!, kl = long.keys.at(-1)!;
        const ends = [short.points.at(-1)!, long.points.at(-1)!];
        const between = edge.map((p, k) => ({ p, k })).filter(({ p, k }) => k > Math.min(from, to) && k < Math.max(from, to) && !ends.some(end => onePoint(p, end, unit)));
        if (from > to) between.reverse();
        const run = between.map(({ p, k }) => ({ point: edgeVertices.get(k) ?? edgeVertices.set(k, vertex(p)).get(k)!, key: ks + ((kl - ks) * (k - from)) / (to - from) }));
        const chain = [...short.points, ...run.map(({ point }) => point)], keys = [...short.keys, ...run.map(({ key }) => key)];
        zipper(list, chain, long.points, [keys, long.keys]);
    };
    for (let i = 0; i + 1 < made.length; i++) strip(b.near, made[i], made[i + 1]);
    const kept = made.filter((_, i) => parts[i].far);
    for (let i = 0; i + 1 < kept.length; i++) strip(b.far, kept[i], kept[i + 1]);
}

/**
 * The outer edge of a way's core on `side` at a station, as the sweep draws it: the last point before its band; on a
 * sidewalk, the top of its edging stone, which a guest meets at the sidewalk's level.
 */
function edgeAt({ line, lift }: Run, s: Station, side: 1 | -1): { x: number; y: number; z: number } {
    const road = carriageway(line, s.h, s.bank), solid = solidOf(sideOf(line, side, s.v, s.h).parts), rim = solid.at(solid.some(({ mat }) => mat === MAT.SIDEWALK) ? -2 : -1), a = side * (rim?.a ?? s.h);
    return { x: s.x + s.ax * a, y: s.y + s.ay * a, z: s.z + s.bank * a + lift + (rim ?? road[side > 0 ? 0 : road.length - 1]).dz };
}

/** A host's outer edge on the side of a point, from its junction out. */
function hostEdge(host: Way, x: number, y: number) {
    const list = outward(host), side = footOn(host, x, y, list).left > 0 ? 1 : -1;
    return { run: host.run, side, list, edge: list.map(s => edgeAt(host.run, s, side)) } as const;
}

type HostEdge = ReturnType<typeof hostEdge>;

/** Where segment p-q first crosses a host's outer edge: the edge, its segment k and the share u along that, and the share t along p-q. */
function meetEdge(edges: HostEdge[], p: { x: number; y: number }, q: { x: number; y: number }) {
    let best: { edge: HostEdge; k: number; u: number; t: number } | undefined;
    for (const edge of edges) for (let k = 0; k + 1 < edge.edge.length; k++) {
        const a = edge.edge[k], c = edge.edge[k + 1], rx = q.x - p.x, ry = q.y - p.y, sx = c.x - a.x, sy = c.y - a.y, cross = rx * sy - ry * sx;
        if (Math.abs(cross) < 1e-12) continue;
        const t = ((a.x - p.x) * sy - (a.y - p.y) * sx) / cross, u = ((a.x - p.x) * ry - (a.y - p.y) * rx) / cross;
        if (t >= 0 && t <= 1 && u >= 0 && u <= 1 && !(best && t >= best.t)) best = { edge, k, u, t };
    }
    return best;
}

/** A point on a host's outer edge: segment k at share u. */
const onEdge = ({ edge }: HostEdge, k: number, u: number) => pointOn(edge, k, u);

/** A point where a guest meets its hosts' outer edge: on segment k of that edge, at share u. */
type Met = { edge: HostEdge; k: number; u: number };

/** The hosts' outer edge from one meet to another: the two points, and between them the stations of one host's run. */
function edgeBetween(P: Met, Q: Met): { x: number; y: number; z: number }[] {
    const metres = ({ edge: { list }, k, u }: Met) => mix(list[k].along, list[k + 1].along, u), [from, to] = [metres(P), metres(Q)], { run, side } = P.edge;
    const inner = Q.edge.run !== run ? [] : run.stations.filter(s => s.along > Math.min(from, to) + SAME_M && s.along < Math.max(from, to) - SAME_M);
    if (from > to) inner.reverse();
    return [onEdge(P.edge, P.k, P.u), ...inner.map(s => edgeAt(run, s, side)), onEdge(Q.edge, Q.k, Q.u)];
}

/**
 * The band of a guest's side along its stretch inside the hosts that lies outside them, from station j to k: the rows
 * on the stamped ground past its core, as the sweep draws them, so the terrain's grid meets the fill as it meets a way.
 */
function band(b: PieceBuilder, run: Run, side: 1 | -1, j: number, k: number): void {
    const { line, stations, prio, lift, ways } = run, code = drawnCode(line);
    const rows = (s: Station, far: boolean) => {
        const { parts, edge } = sideOf(line, side, s.v, s.h), rim = solidOf(parts).at(-1), at = (a: number) => ({ x: s.x + s.ax * side * a, y: s.y + s.ay * side * a });
        const first = at(rim?.a ?? s.h), out = [b.vertex(first.x, first.y, s.z + s.bank * side * (rim?.a ?? s.h) + lift + (rim?.dz ?? LIFT_M), lookWord(rim?.mat ?? MAT.GROUND, code, 0, prio, s.h), side * (rim?.a ?? s.h), 0)];
        // A band's row holds the plane, from which the shader reads the stamped ground.
        for (const part of parts.slice(solidOf(parts).length)) if (!far || part.far) {
            const { x, y } = at(part.a);
            out.push(b.vertex(x, y, s.z + s.bank * side * part.a + lift, lookWord(part.mat, code, under(ways, part.flags, x, y), prio, edge), side * part.a, 0));
        }
        return out;
    };
    for (let m = Math.min(j, k); m < Math.max(j, k); m++) for (const [list, far] of [[b.near, false], [b.far, true]] as const) {
        const S = rows(stations[m], far), T = rows(stations[m + 1], far);
        for (let c = 0; c + 1 < Math.min(S.length, T.length); c++) quad(list, S[c], S[c + 1], T[c + 1], T[c], side > 0);
    }
}

/**
 * A guest's fill, in its own look run on from its carriageway (`ownLook`): the ground of its core, from its stop back to
 * its junction, that lies outside its hosts' outer edge. Each edge of the core that lies outside at the stop runs back
 * until it crosses the hosts' outer edge. Where only one does, the stop's section crosses the outer edge itself. The
 * fill joins that outline to the outer edge.
 */
export function seamFaces(b: PieceBuilder, { way, at, hosts }: Seam): void {
    const { run, step } = way, { line, stations } = run, j = stationIndex(stations, at), T = stations[j], road = carriageway(line, T.h, T.bank);
    const look = ownLook(way, T.h);
    const edges = hosts.map(host => hostEdge(host, T.x, T.y));
    const at0 = (s: Station, a: number) => ({ x: s.x + s.ax * a, y: s.y + s.ay * a });
    // The stop's section across its core, left to right: a side's points by their signed metres across.
    const solid = (side: 1 | -1) => solidOf(sideOf(line, side, T.v, T.h).parts).map(part => ({ part, a: side * part.a }));
    const section = [...solid(1).reverse(), ...road.map(part => ({ part, a: part.a })), ...solid(-1)];
    const vertex = ({ x, y, z }: { x: number; y: number; z: number }): Point => ({ i: surfaceVertex(b, x, y, z, look), x, y, z });
    const own = ({ part, a }: (typeof section)[number]) => vertex({ ...at0(T, a), z: T.z + T.bank * a + run.lift + part.dz });
    /** An edge of the core from the stop back toward the junction, at its own heights, which the stamp holds there, up to where it crosses the hosts' outer edge. */
    const backOf = (side: 1 | -1) => {
        const points: { x: number; y: number; z: number }[] = [], first = stationIndex(stations, way.along), start = edgeAt(run, T, side);
        for (let k = j; k !== first; k -= step) {
            const p = edgeAt(run, stations[k], side), q = edgeAt(run, stations[k - step], side), met = meetEdge(edges, p, q);
            if (met) {
                band(b, run, side, j, k);
                // Between the stop and the outer edge the fill's rim runs from the one height to the other: inside the host the guest's own heights are the host's to draw.
                const end = onEdge(met.edge, met.k, met.u), lengths = [start, ...points, end].map((point, i, all) => (i ? Math.hypot(point.x - all[i - 1].x, point.y - all[i - 1].y) : 0));
                const total = lengths.reduce((a, c) => a + c, 0) || 1;
                let walked = 0;
                return { met, points: [...points.map((point, i) => vertex({ ...point, z: start.z + (end.z - start.z) * ((walked += lengths[i + 1]) / total) })), vertex(end)] };
            }
            points.push(q);
        }
        return undefined;
    };
    const [left, right] = [backOf(1), backOf(-1)];
    // With one edge outside, the stop's section crosses the outer edge between the two.
    const cross = left && right ? undefined : meetEdge(edges, at0(T, section[0].a), at0(T, section[section.length - 1].a));
    if ((!left && !right) || (!(left && right) && !cross)) return;
    const a = cross ? mix(section[0].a, section[section.length - 1].a, cross.t) : 0, C = cross && vertex(onEdge(cross.edge, cross.k, cross.u));
    const kept = section.filter(point => (left && right) || (left ? point.a >= a : point.a <= a));
    const seam = edgeBetween(left?.met ?? cross!, right?.met ?? cross!).map(vertex);
    const outline = (points: typeof section) => [...(left ? left.points.slice().reverse() : [C!]), ...points.map(own), ...(right ? right.points : [C!])];
    zipper(b.near, outline(kept), seam);
    zipper(b.far, outline(kept.filter(({ part }) => part.far)), seam);
}
