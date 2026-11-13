/** Groups the rides that follow one road: a ride joins the planned route or the first ride of a group whose line it follows. */
import type { RideRecord, RouteRecord, Summary } from './records';

// Start values, to test on real rides: two lines are on one road when their lengths differ by 10 % or less and 90 % of
// each is within 50 m of the other. A summary line is within 40 m of its points, so its vertices test at 90 m.
const LENGTH_SHARE = 0.1, MISS_SHARE = 0.1, NEAR_M = 50 + 40;
// One comparison takes about 1 ms, so a turn after 40 ms of work keeps each task under 50 ms.
const WORK_MS = 40;
const METRES_PER_DEGREE = 6_371_008.8 * Math.PI / 180;

/** Two summaries are of one road. The start point and the direction do not matter. */
export function sameRoute(a: Summary, b: Summary): boolean {
    const longer = Math.max(a.lengthM, b.lengthM);
    if (a.line.length < 4 || b.line.length < 4 || Math.abs(a.lengthM - b.lengthM) > LENGTH_SHARE * longer) return false;
    // Lines of one road have near bounds: each edge moves by at most a tenth of the length, plus the tolerance.
    const k = Math.cos((a.bounds[1] + a.bounds[3]) / 2 * Math.PI / 180), edge = (LENGTH_SHARE * longer + NEAR_M) / METRES_PER_DEGREE;
    if (a.bounds.some((value, i) => Math.abs(value - b.bounds[i]) * (i % 2 ? 1 : k) > edge)) return false;
    // One flat projection for both lines, at the first point of `a`.
    const project = (line: number[]) => line.map((value, i) => (value - a.line[i % 2]) * METRES_PER_DEGREE * (i % 2 ? 1 : k));
    const pa = project(a.line), pb = project(b.line);
    return covered(pa, pb) && covered(pb, pa);
}

/** At most a tenth of `line`, each vertex weighted by half of its two segments, is farther than `NEAR_M` from `other`. */
function covered(line: number[], other: number[]): boolean {
    const grid = cells(other), n = line.length / 2;
    const step = (i: number) => Math.hypot(line[2 * i + 2] - line[2 * i], line[2 * i + 3] - line[2 * i + 1]);
    let total = 0;
    for (let i = 0; i + 1 < n; i++) total += step(i);
    let missed = 0;
    for (let i = 0; i < n; i++) {
        const weight = ((i ? step(i - 1) : 0) + (i + 1 < n ? step(i) : 0)) / 2;
        if (!weight || near(line[2 * i], line[2 * i + 1], other, grid)) continue;
        missed += weight;
        if (missed > MISS_SHARE * total) return false;
    }
    return true;
}

const cellOf = (value: number) => Math.floor(value / NEAR_M);
const cellKey = (x: number, y: number) => x * 0x100000 + y;

/** The segments of a line by the cells of `NEAR_M` that their points, one each `NEAR_M` along them, fall in. */
function cells(line: number[]): Map<number, number[]> {
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
function near(x: number, y: number, line: number[], grid: Map<number, number[]>): boolean {
    const cx = cellOf(x), cy = cellOf(y);
    for (let dx = -2; dx <= 2; dx++) {
        for (let dy = -2; dy <= 2; dy++) {
            for (const s of grid.get(cellKey(cx + dx, cy + dy)) ?? []) {
                const ax = line[s], ay = line[s + 1], ex = line[s + 2] - ax, ey = line[s + 3] - ay, length = ex * ex + ey * ey;
                const t = length ? Math.max(0, Math.min(1, ((x - ax) * ex + (y - ay) * ey) / length)) : 0;
                if (Math.hypot(x - ax - t * ex, y - ay - t * ey) <= NEAR_M) return true;
            }
        }
    }
    return false;
}

/** The results of `sameRoute` for this session, by ride key and route id with its time of change. */
const known = new Map<string, boolean>();

/**
 * The group of each ride that `apart` does not hold, by ride key: the id of a route, or the key of the first ride of the
 * group. In start order, a ride joins the first route it follows, newest first, else the group of the first ride it
 * follows, else it starts a group. A group holds the ride that started it, so a group of one ride has its key.
 */
export async function groups(rides: RideRecord[], routes: RouteRecord[], apart: string[] = []): Promise<Map<string, string>> {
    const out = new Map<string, string>(), skip = new Set(apart), firsts: RideRecord[] = [];
    const anchors = routes.filter(route => route.summary).sort((a, b) => b.updatedAt - a.updatedAt);
    let since = performance.now();
    const same = async (ride: RideRecord, id: string, summary: Summary) => {
        if (performance.now() - since > WORK_MS) {
            await new Promise(resolve => setTimeout(resolve));
            since = performance.now();
        }
        const pair = `${ride.key}|${id}`;
        let result = known.get(pair);
        if (result === undefined) known.set(pair, result = sameRoute(ride.summary, summary));
        return result;
    };
    for (const ride of [...rides].sort((a, b) => a.summary.start - b.summary.start)) {
        if (skip.has(ride.key)) continue;
        let group: string | undefined;
        for (const route of anchors) if (!group && await same(ride, `${route.id}@${route.updatedAt}`, route.summary!)) group = route.id;
        for (const first of firsts) if (!group && await same(ride, first.key, first.summary)) group = first.key;
        if (!group) firsts.push(ride);
        out.set(ride.key, group ?? ride.key);
    }
    return out;
}
