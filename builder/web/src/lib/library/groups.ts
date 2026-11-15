/** Groups the rides that follow one road: a ride joins the planned route or the first ride of a group whose line it follows. */
import type { RideRecord, RouteRecord, Summary } from './records';
import { lastToleranceM, metresPerDegree } from './summary';

// Two lines are on one road when their lengths differ by 10 % or less and 90 % of each is within 50 m of the other. A
// summary line is within 40 m of its points, so its vertices test at 90 m.
const LENGTH_SHARE = 0.1, MISS_SHARE = 0.1, NEAR_M = 50 + lastToleranceM;
// A task of the event loop ends after this much work, so a full test that starts just before it ends under 50 ms.
const WORK_MS = 30;

/** A summary line on a flat projection at its first point, in metres, its segments by grid cell, and the weight of each
 * vertex: half of its two segments. */
interface Prepared {
    lon0: number;
    lat0: number;
    k: number;
    xy: Float64Array;
    grid: Map<number, number[]>;
    weights: Float64Array;
    total: number;
}

function prepare({ line }: Summary): Prepared {
    const lon0 = line[0], lat0 = line[1], k = Math.cos(lat0 * Math.PI / 180) * metresPerDegree;
    const xy = Float64Array.from(line, (value, i) => i % 2 ? (value - lat0) * metresPerDegree : (value - lon0) * k);
    const n = xy.length / 2, weights = new Float64Array(n);
    let total = 0;
    for (let i = 0; i + 1 < n; i++) {
        const step = Math.hypot(xy[2 * i + 2] - xy[2 * i], xy[2 * i + 3] - xy[2 * i + 1]);
        weights[i] += step / 2;
        weights[i + 1] += step / 2;
        total += step;
    }
    return { lon0, lat0, k, xy, grid: cells(xy), weights, total };
}

const middles = new WeakMap<Summary, [number, number]>();

/** The mean point of a summary line, each segment weighted by its length, in degrees. */
function middle(summary: Summary): [number, number] {
    let point = middles.get(summary);
    if (point) return point;
    const { line } = summary, k = Math.cos(line[1] * Math.PI / 180);
    let x = 0, y = 0, total = 0;
    for (let i = 2; i < line.length; i += 2) {
        const length = Math.hypot((line[i] - line[i - 2]) * k, line[i + 1] - line[i - 1]);
        [x, y, total] = [x + (line[i] + line[i - 2]) / 2 * length, y + (line[i + 1] + line[i - 1]) / 2 * length, total + length];
    }
    middles.set(summary, point = total ? [x / total, y / total] : [line[0], line[1]]);
    return point;
}

/** The lengths are within 10 %, and each bounds edge and the mean point move by at most a tenth of the length, the
 * tolerance, and the 40 m by which the bounds of the points can pass the summary line. A tenth of a line elsewhere, at
 * most half a length away, moves the mean point by a twentieth of the length; the other line can move it as far. */
function near(a: Summary, b: Summary): boolean {
    const longer = Math.max(a.lengthM, b.lengthM);
    if (a.line.length < 4 || b.line.length < 4 || Math.abs(a.lengthM - b.lengthM) > LENGTH_SHARE * longer) return false;
    const k = Math.cos((a.bounds[1] + a.bounds[3]) / 2 * Math.PI / 180), edge = (LENGTH_SHARE * longer + NEAR_M + lastToleranceM) / metresPerDegree;
    if (!a.bounds.every((value, i) => Math.abs(value - b.bounds[i]) * (i % 2 ? 1 : k) <= edge)) return false;
    const [ma, mb] = [middle(a), middle(b)];
    return Math.hypot((ma[0] - mb[0]) * k, ma[1] - mb[1]) <= edge;
}

/** Two summaries are of one road. The start point and the direction do not matter. */
export function sameRoute(a: Summary, b: Summary): boolean {
    return near(a, b) && full(a, b, prepare(a), prepare(b));
}

const full = (a: Summary, b: Summary, pa: Prepared, pb: Prepared) => covered(a.line, pa, pb) && covered(b.line, pb, pa);

/** At most a tenth of `line`, by the weights of its vertices, is farther than `NEAR_M` from `other`. */
function covered(line: number[], { weights, total }: Prepared, other: Prepared): boolean {
    let missed = 0;
    for (let i = 0; i < weights.length; i++) {
        const weight = weights[i];
        // The vertex in the projection of the other line.
        const x = (line[2 * i] - other.lon0) * other.k, y = (line[2 * i + 1] - other.lat0) * metresPerDegree;
        if (!weight || within(x, y, other)) continue;
        missed += weight;
        if (missed > MISS_SHARE * total) return false;
    }
    return true;
}

const cellOf = (value: number) => Math.floor(value / NEAR_M);
const cellKey = (x: number, y: number) => x * 0x100000 + y;

/** The segments of a line by the cells of `NEAR_M` that their points, one each `NEAR_M` along them, fall in. */
function cells(line: Float64Array): Map<number, number[]> {
    const grid = new Map<number, number[]>();
    for (let s = 0; s + 3 < line.length; s += 2) {
        const [ax, ay, bx, by] = [line[s], line[s + 1], line[s + 2], line[s + 3]];
        const steps = Math.ceil(Math.hypot(bx - ax, by - ay) / NEAR_M);
        let last = NaN;
        for (let j = 0; j <= steps; j++) {
            const t = steps ? j / steps : 0, key = cellKey(cellOf(ax + t * (bx - ax)), cellOf(ay + t * (by - ay)));
            if (key === last) continue;
            last = key;
            const list = grid.get(key);
            if (!list) grid.set(key, [s]);
            else if (list.at(-1) !== s) list.push(s);
        }
    }
    return grid;
}

/** The point is within `NEAR_M` of a segment. The nearest point of a segment is within half a cell of one of its cell
 * points, so the segment is in a cell at most two away. */
function within(x: number, y: number, { xy, grid }: Prepared): boolean {
    const cx = cellOf(x), cy = cellOf(y);
    for (let dx = -2; dx <= 2; dx++) {
        for (let dy = -2; dy <= 2; dy++) {
            for (const s of grid.get(cellKey(cx + dx, cy + dy)) ?? []) {
                const ax = xy[s], ay = xy[s + 1], ex = xy[s + 2] - ax, ey = xy[s + 3] - ay, length = ex * ex + ey * ey;
                const t = length ? Math.max(0, Math.min(1, ((x - ax) * ex + (y - ay) * ey) / length)) : 0;
                if (Math.hypot(x - ax - t * ex, y - ay - t * ey) <= NEAR_M) return true;
            }
        }
    }
    return false;
}

/** The results of the full test for this session, by ride key and route id with its time of change. Only pairs that pass
 * the cheap test reach it. */
const known = new Map<string, boolean>();

/**
 * The group of each ride that `apart` does not hold, by ride key: the id of a route, or the key of the first ride of the
 * group. In start order, a ride joins the first route it follows, newest first, else the group of the first ride it
 * follows, else it starts a group. A group holds the ride that started it, so a group of one ride has its key.
 */
export async function groups(rides: RideRecord[], routes: RouteRecord[], apart: string[] = []): Promise<Map<string, string>> {
    const out = new Map<string, string>(), skip = new Set(apart), firsts: { id: string; pair: string; summary: Summary }[] = [];
    const anchors = routes.filter(route => route.summary).sort((a, b) => b.updatedAt - a.updatedAt)
        .map(route => ({ id: route.id, pair: `${route.id}@${route.updatedAt}`, summary: route.summary! }));
    // The projections of this run; a run frees them, so only the results stay.
    const prepared = new Map<Summary, Prepared>(), of = (summary: Summary) => prepared.get(summary) ?? prepared.set(summary, prepare(summary)).get(summary)!;
    let since = performance.now();
    const same = async (ride: RideRecord, pair: string, summary: Summary) => {
        const key = `${ride.key}|${pair}`;
        let result = known.get(key);
        if (result === undefined) {
            if (performance.now() - since > WORK_MS) {
                await new Promise(resolve => setTimeout(resolve));
                since = performance.now();
            }
            known.set(key, result = full(ride.summary, summary, of(ride.summary), of(summary)));
        }
        return result;
    };
    for (const ride of [...rides].sort((a, b) => a.summary.start - b.summary.start)) {
        if (skip.has(ride.key)) continue;
        let group: string | undefined;
        for (const { id, pair, summary } of [...anchors, ...firsts]) {
            if (near(ride.summary, summary) && await same(ride, pair, summary)) {
                group = id;
                break;
            }
        }
        if (!group) firsts.push({ id: ride.key, pair: ride.key, summary: ride.summary });
        out.set(ride.key, group ?? ride.key);
    }
    return out;
}
