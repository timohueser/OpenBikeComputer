// A path that OSM maps along or across a paved road, inside its core: it is the road's sidewalk or a crossing. The bake holds
// its heights apart from the road's, so it would run under the road's faces and vanish. Here it lies on the road's surface,
// as far as its middle lies on the road, and returns to its own heights past that. The corridors draw no tread there.
import { alongOf, cutLine, pointAlong, RETURN_CHORDS, RETURN_GRADE, RETURN_LEAST_M, SAME_M } from './approaches';
import { KERB_RISE_M, metresPerUnit, PATH, PROFILES, ROAD, SHOULDER_M, SURFACES, walkOf, type Line } from './bins';
import { gridOf, LEFT, near, RIGHT } from './town';

/** The step of the search along a path; a gap between two stretches this short is none; points this near a stretch's end lie on it. */
const STEP_M = 0.25, GAP_M = 6, NEAR_M = 0.05;
/** A way this far off a road's surface passes it, on a level of its own. */
const APART_M = 4;
/** A return takes at most this many metres. */
const MOST_M = 16;

const PAVED = ['main', 'road', 'oneway', 'paving'];
const surfaceOf = (line: Line) => (Object.keys(SURFACES) as (keyof typeof SURFACES)[])[(line.code & 15) - 1];
const shoulderOf = (line: Line) => SHOULDER_M[surfaceOf(line)] ?? 0;
const smooth = (t: number) => t * t * (3 - 2 * t);

/** A segment of a paved road's line. */
interface Span { line: Line; v: number }

/** The roads of a tile in a grid: past a carriageway a sidewalk or a shoulder holds its plane too. */
function roadsOf(lines: Line[]) {
    const spans: Span[] = [];
    for (const line of lines) {
        if (line.kind !== ROAD || line.gallery || !PAVED.includes(surfaceOf(line))) continue;
        for (let v = 0; v + 1 < line.z.length; v++) spans.push({ line, v });
    }
    const reach = ({ line }: Span) => line.h + Math.max(walkOf(line), shoulderOf(line));
    return {
        spans,
        grid: gridOf(spans.map(span => {
            const { line: { xy }, v } = span, r = reach(span) / metresPerUnit(xy[2 * v + 1]);
            return [Math.min(xy[2 * v], xy[2 * v + 2]) - r, Math.min(xy[2 * v + 1], xy[2 * v + 3]) - r, Math.max(xy[2 * v], xy[2 * v + 2]) + r, Math.max(xy[2 * v + 1], xy[2 * v + 3]) + r];
        })),
    };
}

/** The height of the surface of the roads at a world point, the highest where two hold it; undefined off them. */
function surfaceOn({ spans, grid }: ReturnType<typeof roadsOf>, x: number, y: number): number | undefined {
    const unit = metresPerUnit(y);
    let best: number | undefined;
    near(grid, x, y, x, y, 0, s => {
        const { line, v } = spans[s], [ax, ay, bx, by] = line.xy.slice(2 * v, 2 * v + 4), dx = bx - ax, dy = by - ay;
        // A segment holds only the points whose foot lies on it, as footOn of approaches.ts: so a path that runs on from a road's end, or
        // past the end of a short last segment, stays off the road.
        const t = ((x - ax) * dx + (y - ay) * dy) / (dx * dx + dy * dy || 1);
        if (t < 0 || t > 1) return;
        // Left of the line: y runs south.
        const left = ((x - ax) * dy - (y - ay) * dx) / (Math.hypot(dx, dy) || 1) * unit, walks = line.sidewalks?.[v] ?? 0, walk = walks & (left > 0 ? LEFT : RIGHT) ? walkOf(line) : 0;
        if (Math.abs(left) >= line.h + (walk || shoulderOf(line))) return;
        const bank = line.plan ? line.plan.bank[v] + t * (line.plan.bank[v + 1] - line.plan.bank[v]) : 0;
        const z = line.z[v] + t * (line.z[v + 1] - line.z[v]) + bank * Math.max(-line.h, Math.min(line.h, left)) + (Math.abs(left) > line.h && walk ? KERB_RISE_M : 0);
        if (best === undefined || z > best) best = z;
    });
    return best;
}

/**
 * The stretches of a line, in metres along it, whose middle lies on a road near its heights. It samples each segment alone, from its
 * first vertex, so two tiles that hold a segment agree; a short gap between two stretches is none. A single point, such as a vertex that
 * the path shares with a road's end, is no stretch.
 */
function stretchesOf(line: Line, metres: number[], roads: ReturnType<typeof roadsOf>): [number, number][] {
    const out: [number, number][] = [];
    let from: number | undefined, last = 0;
    const take = (at: number) => {
        const p = pointAlong(line, metres, at), z = surfaceOn(roads, p.x, p.y);
        if (z === undefined || Math.abs(z - (line.z[p.v] + p.u * (line.z[p.v + 1] - line.z[p.v]))) >= APART_M) {
            if (from === undefined) return;
            out.push([from, last]);
            from = undefined;
        } else {
            from ??= at;
            last = at;
        }
    };
    for (let v = 0; v + 1 < metres.length; v++) {
        // A segment that no road's box reaches holds no point on a road: its first sample alone ends a stretch.
        let reached = false;
        near(roads.grid, line.xy[2 * v], line.xy[2 * v + 1], line.xy[2 * v + 2], line.xy[2 * v + 3], 0, () => (reached = true));
        const n = reached ? Math.ceil((metres[v + 1] - metres[v]) / STEP_M) : 1;
        for (let k = 0; k < n; k++) take(metres[v] + (k * (metres[v + 1] - metres[v])) / n);
    }
    take(metres[metres.length - 1]);
    if (from !== undefined) out.push([from, last]);
    return out.reduce<[number, number][]>((kept, stretch) => {
        const held = kept[kept.length - 1];
        if (held && stretch[0] - held[1] <= GAP_M) held[1] = stretch[1];
        else kept.push(stretch);
        return kept;
    }, []).filter(([a, b]) => b > a);
}

/** Lays the paths that lie on paved roads on those roads' surfaces, and notes the stretches in `ridden`. */
export function riders(lines: Line[]): void {
    const roads = roadsOf(lines);
    if (!roads.spans.length) return;
    const name = (line: Line, v: number) => `${line.xy[2 * v]},${line.xy[2 * v + 1]}`;
    let seen: Map<string, number> | undefined;
    for (const line of lines) {
        if (line.kind !== PATH || line.gallery || Object.keys(PROFILES)[line.profile] === 'steps' || line.z.length < 2) continue;
        const metres = alongOf(line), stretches = stretchesOf(line, metres, roads);
        if (!stretches.length) continue;
        if (!seen) {
            seen = new Map();
            for (const other of lines) other.z.forEach((_, v) => seen!.set(name(other, v), (seen!.get(name(other, v)) ?? 0) + 1));
        }
        // The next point past m where other heights hold: a shared vertex, another stretch or the line's end.
        const stops = metres.filter((_, v) => (seen!.get(name(line, v)) ?? 0) > 1 || v === 0 || v === metres.length - 1);
        const room = (m: number, dir: 1 | -1) => Math.min(...[...stops, ...stretches.flat()].map(stop => (stop - m) * dir).filter(d => d > 1e-3), MOST_M);
        // Each end of a stretch takes the road's height, one step inside it; its ramp runs out to where the path's own heights take over.
        // As editOf of approaches.ts, a short room limits the change to RETURN_GRADE over it: the end then stops short of the road's height.
        const ramps = stretches.flatMap(([a, b]) => ([[a, -1], [b, 1]] as const).map(([m, dir]) => {
            const p = pointAlong(line, metres, m), wanted = surfaceOn(roads, p.x, p.y)! - (line.z[p.v] + p.u * (line.z[p.v + 1] - line.z[p.v]));
            const free = room(m, dir), most = RETURN_GRADE * free, change = Math.max(-most, Math.min(most, wanted));
            return { m, dir, change, held: change !== wanted, over: Math.min(free, Math.max(RETURN_LEAST_M, Math.abs(change) / RETURN_GRADE)) };
        }));
        // Chords inside each return, so the stamp's straight segments follow its curve.
        cutLine(line, ramps.flatMap(({ m, dir, over }) => Array.from({ length: RETURN_CHORDS + 1 }, (_, k) => m + (dir * over * k) / RETURN_CHORDS)), metres);
        const after = alongOf(line), own = line.z.slice(), on = (m: number) => stretches.find(([a, b]) => m >= a - NEAR_M && m <= b + NEAR_M);
        const surface = own.map((_, v) => {
            const { x, y } = pointAlong(line, after, after[v]);
            return on(after[v]) ? surfaceOn(roads, x, y) : undefined;
        });
        line.z = own.map((z, v) => {
            const end = ramps.find(({ m, held }) => held && Math.abs(after[v] - m) <= NEAR_M);
            if (end) return z + end.change;
            if (on(after[v])) {
                if (surface[v] !== undefined) return surface[v]!;
                // In a gap the height runs from one side to the other.
                let [p, q] = [v, v];
                while (p > 0 && surface[p] === undefined) p--;
                while (q + 1 < own.length && surface[q] === undefined) q++;
                return (surface[p] ?? z) + ((surface[q] ?? z) - (surface[p] ?? z)) * (after[v] - after[p]) / ((after[q] - after[p]) || 1);
            }
            const ramp = ramps.find(({ m, dir, over }) => (after[v] - m) * dir > 0 && (after[v] - m) * dir < over);
            return ramp && Math.abs(ramp.change) >= SAME_M ? z + ramp.change * (1 - smooth(Math.abs(after[v] - ramp.m) / ramp.over)) : z;
        });
        if (line.plan) for (let v = 1; v + 1 < line.z.length; v++) if (line.z[v - 1] !== own[v - 1] || line.z[v] !== own[v] || line.z[v + 1] !== own[v + 1]) line.plan.grade[v] = (line.z[v + 1] - line.z[v - 1]) / (after[v + 1] - after[v - 1] || 1);
        line.ridden = stretches;
    }
}
