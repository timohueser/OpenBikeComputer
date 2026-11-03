// The pivots of a line's tight bends: where the inner edge of its core (the carriageway and its shoulder) would fold,
// the edge runs round an arc about a pivot instead, and the inner cross-sections of the bend point at that pivot. The
// stamp (stamp-core.wgsl) and the corridor's sweep (roads/sweep.ts) both read them, so they hold the same ground.
import { metresPerUnit, PATH, RAIL, ROAD, shoulderOf, type Line } from './bins';

/**
 * A pivot: the inner side of its bend (1 left, -1 right), its point in world units, the radius of the core's inner edge
 * about it in metres, and the line's vertices whose inner cross-sections point at it, `first` to `last`.
 */
export interface Pivot { side: 1 | -1; x: number; y: number; r: number; first: number; last: number }

/**
 * The radii of an inner edge, largest first: a bend takes the largest whose fill keeps the inner edge's mean grade
 * under STEEPEST, else the least steep up to STEEP_MOST. Past that the fill would stand as a wall, and the bend keeps its
 * fold (sweep.ts): chosen against the torn faces of the fixture, which 0.6, 0.8 and 1.5 raise. A way narrower than a
 * radius on each side does not take it.
 */
export const RADII = [1.5, 1.05, 0.75, 0.5];
const STEEPEST = 0.35, STEEP_MOST = 1;
/** A pivot lies within this many metres of each vertex of its fan and the vertex on each side, as `fanWord` packs it in steps of PIVOT_STEP_M. */
export const PIVOT_RANGE_M = 20, PIVOT_STEP_M = 0.02;
/** A fan word's marks of the ends whose inner cross-sections point at the pivot, as FAN_A and FAN_B of stamp-core.wgsl. */
export const FAN_A = 2 ** 30, FAN_B = 2 ** 31;
/** What blocks an offset point is looked for this many metres along the line on each side. */
const WINDOW_M = 30;
/** The most that a chord leaves the curve, as ROAD_SAG and PATH_SAG of stream/curve.ts: an offset point nearer a chord by less is free. */
const SAG_M = 0.05, PATH_SAG_M = 0.15;

/** A line's vertices in world units about its first, their units per metre and normals, the metres along, the heights and the core's half-widths. */
interface Frame { x: number[]; y: number[]; unit: number[]; nx: number[]; ny: number[]; along: number[]; z: number[]; h: number[]; sag: number }
type Fan = Pivot & { steep: number };

const known = new WeakMap<Line, Pivot[]>();

/** The pivots of a line, made once; `half` gives the carriageway's half-width at a vertex. A fan's side has no sidewalk, so its core ends at the shoulder. */
export function pivotsOf(line: Line, half: (v: number) => number): Pivot[] {
    const had = known.get(line);
    if (had) return had;
    const out = line.plan && line.z.length > 4 && (line.kind === ROAD || line.kind === RAIL || line.kind === PATH) && !line.gallery ? find(line, half) : [];
    known.set(line, out);
    return out;
}

function find(line: Line, half: (v: number) => number): Pivot[] {
    const [x0, y0] = line.xy, shoulder = shoulderOf(line), f: Frame = { x: [], y: [], unit: [], nx: [], ny: [], along: [], z: line.z, h: [], sag: line.kind === PATH ? PATH_SAG_M : SAG_M };
    line.z.forEach((_, v) => {
        const t = line.plan!.tangent[v];
        f.x.push(line.xy[2 * v] - x0);
        f.y.push(line.xy[2 * v + 1] - y0);
        f.unit.push(1 / metresPerUnit(line.xy[2 * v + 1]));
        // To the left of the way, as a station's frame: world y runs south.
        f.nx.push(Math.sin(t));
        f.ny.push(-Math.cos(t));
        f.along.push(v ? f.along[v - 1] + Math.hypot(f.x[v] - f.x[v - 1], f.y[v] - f.y[v - 1]) * 2 / (f.unit[v] + f.unit[v - 1]) : 0);
        f.h.push(half(v) + shoulder);
    });
    // A segment and a station hold one fan: two fans, on either side, keep a vertex apart.
    const chosen: Fan[] = [], steep: Fan[] = [], apart = (fan: Fan) => !chosen.some(other => other.first <= fan.last + 1 && fan.first <= other.last + 1);
    for (const r of RADII.filter(r => r <= line.h)) for (const side of [1, -1] as const) for (const fan of fans(f, side, r)) if (apart(fan)) (fan.steep <= STEEPEST ? chosen : steep).push(fan);
    for (const fan of steep.sort((a, b) => a.steep - b.steep)) if (fan.steep <= STEEP_MOST && apart(fan)) chosen.push(fan);
    return chosen.map(({ side, x, y, r, first, last }) => ({ side, x: x0 + x, y: y0 + y, r, first, last })).sort((a, b) => a.first - b.first);
}

/**
 * The fan word of a line's segment from vertex v, as `along` of stamp-core.wgsl reads it: the pivot about the segment's
 * start in steps of PIVOT_STEP_M (11 bits each, x then y), the arc's radius in cm (8 bits), and whether the inner
 * cross-section of the start and of the end points at the pivot (bits 30 and 31). 0 where neither does.
 */
export function fanWord(line: Line, pivots: Pivot[], v: number): number {
    const pivot = pivots.find(({ first, last }) => v + 1 >= first && v <= last);
    if (!pivot) return 0;
    const [x, y] = line.xy.slice(2 * v, 2 * v + 2), m = metresPerUnit(y), step = (d: number) => Math.round((d * m) / PIVOT_STEP_M) & 0x7ff;
    return (step(pivot.x - x) | (step(pivot.y - y) << 11) | (Math.round(pivot.r * 100) << 22)) + (v >= pivot.first ? FAN_A : 0) + (v + 1 <= pivot.last ? FAN_B : 0);
}

/** The point where segments p0-p1 and q0-q1 cross, if they do. */
function crossing(p0x: number, p0y: number, p1x: number, p1y: number, q0x: number, q0y: number, q1x: number, q1y: number): [number, number] | undefined {
    const ex = p1x - p0x, ey = p1y - p0y, fx = q1x - q0x, fy = q1y - q0y, den = ex * fy - ey * fx;
    if (Math.abs(den) < 1e-12) return undefined;
    const t = ((q0x - p0x) * fy - (q0y - p0y) * fx) / den, u = ((q0x - p0x) * ey - (q0y - p0y) * ex) / den;
    return t >= 0 && t <= 1 && u >= 0 && u <= 1 ? [p0x + t * ex, p0y + t * ey] : undefined;
}

/**
 * The fans of a side at inner radius r. The offset of the line at h + r folds where the bend is tighter than that.
 * Each fold starts a run of vertices whose offset points lie nearer than h + r to the line: the loop of the offset.
 * The pivot is where the offset crosses itself as it enters and leaves the loop: a corner of the ground that the
 * core and a margin of r leave free. The fan is the run and a vertex on each side of it. It counts only where
 * its pivot lies h + r or more from every vertex near it, its vertices turn about the pivot in one sense, and the
 * cross-sections next to it cross none of its own.
 */
function fans(f: Frame, side: 1 | -1, r: number): Fan[] {
    const n = f.x.length, rho = f.h.map((h, v) => (h + r) * f.unit[v]), sag = (v: number) => f.sag * f.unit[v];
    const ox = f.x.map((x, v) => x + side * f.nx[v] * rho[v]), oy = f.y.map((y, v) => y + side * f.ny[v] * rho[v]);
    const back = (v: number) => (ox[v + 1] - ox[v]) * (f.x[v + 1] - f.x[v]) + (oy[v + 1] - oy[v]) * (f.y[v + 1] - f.y[v]) < 0;
    /** Whether vertex v's offset point lies within h + r of a chord of the line but its own two. */
    const blocked = (v: number) => {
        for (let k = 0; k + 1 < n; k++) {
            if (k === v || k === v - 1 || Math.abs(f.along[k] - f.along[v]) > WINDOW_M) continue;
            const dx = f.x[k + 1] - f.x[k], dy = f.y[k + 1] - f.y[k], t = Math.max(0, Math.min(1, ((ox[v] - f.x[k]) * dx + (oy[v] - f.y[k]) * dy) / (dx * dx + dy * dy || 1)));
            if (Math.hypot(ox[v] - f.x[k] - t * dx, oy[v] - f.y[k] - t * dy) < rho[k] + (rho[k + 1] - rho[k]) * t - sag(k)) return true;
        }
        return false;
    };
    // The runs of the loops, each from a fold out to the last vertex whose offset point is blocked.
    const runs: [number, number][] = [];
    for (let v = 0; v + 1 < n; v++) {
        if (!back(v)) continue;
        let [first, last] = [v, v + 1];
        while (first > 0 && blocked(first - 1)) first--;
        while (last + 1 < n && blocked(last + 1)) last++;
        runs.push([first, last]);
        v = last;
    }
    /** The pivot of a run: the offset enters the loop on the step before it and leaves it on the step after, else on the nearest pair that cross. */
    const pivot = ([first, last]: [number, number]) => {
        for (let d = 0; d < 3; d++) {
            for (let a = 0; a <= d; a++) {
                const i = first - 1 - a, l = last + d - a, q = i >= 0 && l + 1 < n ? crossing(ox[i], oy[i], ox[i + 1], oy[i + 1], ox[l], oy[l], ox[l + 1], oy[l + 1]) : undefined;
                if (q) return q;
            }
        }
        return undefined;
    };
    const out: Fan[] = [];
    for (let k = 0; k < runs.length; k++) {
        let [first, last] = runs[k], q = pivot(runs[k]);
        // Two loops whose pivots lie less than r apart are one bend: no disc of radius r rolls between them. Their
        // pivot is where the offset crosses itself about both, else halfway between theirs.
        for (let next = runs[k + 1] && pivot(runs[k + 1]); q && next && Math.hypot(next[0] - q[0], next[1] - q[1]) < r * f.unit[last]; next = runs[k + 1] && pivot(runs[k + 1])) {
            [last, q] = [runs[++k][1], pivot([first, runs[k][1]]) ?? [(q[0] + next[0]) / 2, (q[1] + next[1]) / 2]];
        }
        [first, last] = [first - 1, last + 1];
        if (!q || first < 1 || last + 2 > n) continue;
        const [qx, qy] = q;
        let free = true;
        for (let w = 0; w < n && free; w++) if (Math.abs(f.along[w] - f.along[first]) <= 2 * WINDOW_M) free = Math.hypot(f.x[w] - qx, f.y[w] - qy) >= rho[w] - sag(w);
        /** The inner cross-section of vertex w out to the core's edge: to the pivot's arc in the fan, else along its normal. */
        const rung = (w: number): [number, number, number, number] => {
            if (w < first || w > last) return [f.x[w], f.y[w], f.x[w] + side * f.nx[w] * f.h[w] * f.unit[w], f.y[w] + side * f.ny[w] * f.h[w] * f.unit[w]];
            const d = Math.hypot(qx - f.x[w], qy - f.y[w]), l = Math.max(0, d - r * f.unit[w]) / d;
            return [f.x[w], f.y[w], f.x[w] + (qx - f.x[w]) * l, f.y[w] + (qy - f.y[w]) * l];
        };
        const angle = (w: number) => Math.atan2(f.y[w] - qy, f.x[w] - qx);
        let turn = 0, sense = 0, ordered = !crossing(...rung(first - 1), ...rung(first)) && !crossing(...rung(last), ...rung(last + 1)), near = true;
        for (let w = first - 1; w <= last + 1; w++) {
            near &&= Math.hypot(f.x[w] - qx, f.y[w] - qy) <= PIVOT_RANGE_M * f.unit[w];
            if (w < first || w >= last) continue;
            const d = Math.atan2(Math.sin(angle(w + 1) - angle(w)), Math.cos(angle(w + 1) - angle(w)));
            sense ||= Math.sign(d);
            ordered &&= Math.sign(d) === sense;
            turn += Math.abs(d);
        }
        if (free && ordered && near) out.push({ side, x: qx, y: qy, r, first, last, steep: Math.abs(f.z[last] - f.z[first]) / (r * Math.max(turn, 1e-3)) });
    }
    return out;
}
