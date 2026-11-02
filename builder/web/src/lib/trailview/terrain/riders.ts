// A path that OSM maps along or across a paved road, inside its core: it is the road's sidewalk or a crossing. The bake holds
// its heights apart from the road's, so it would run under the road's faces and vanish. Here it lies on the road's surface,
// as far as its middle lies on the road, and returns to its own heights past that. The corridors draw no tread there.
import { alongOf, cutLine, pointAlong } from './approaches';
import { KERB_RISE_M, metresPerUnit, PATH, PROFILES, ROAD, SHOULDER_M, SURFACES, walkOf, type Line } from './bins';
import { along, gridOf, LEFT, near, RIGHT } from './town';

/** The step of the search along a path; a gap between two stretches this short is none; points this near a stretch's end lie on it. */
const STEP_M = 0.25, GAP_M = 6, NEAR_M = 0.05;
/** A way this far off a road's surface passes it, on a level of its own. */
const APART_M = 4;
/** A path returns to its own heights at this grade at most, over this many metres at least; a change under SAME_M stays out. */
const RETURN_GRADE = 0.15, RETURN_LEAST_M = 2, MOST_M = 16, SAME_M = 0.01;

const PAVED = ['main', 'road', 'oneway', 'paving'];
const surfaceOf = (line: Line) => (Object.keys(SURFACES) as (keyof typeof SURFACES)[])[(line.code & 15) - 1];
const shoulderOf = (line: Line) => SHOULDER_M[surfaceOf(line)] ?? 0;
const smooth = (t: number) => t * t * (3 - 2 * t);

/** The carriageway of a paved road, with what lies past it, in a segment of a line. */
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
        const { line, v } = spans[s], [ax, ay, bx, by] = line.xy.slice(2 * v, 2 * v + 4), t = along(ax, ay, bx, by, x, y), fx = ax + t * (bx - ax), fy = ay + t * (by - ay);
        // Left of the line: y runs south.
        const dx = bx - ax, dy = by - ay, left = ((x - ax) * dy - (y - ay) * dx) / (Math.hypot(dx, dy) || 1) * unit, walks = line.sidewalks?.[v] ?? 0, walk = walks & (left > 0 ? LEFT : RIGHT) ? walkOf(line) : 0;
        if (Math.hypot(x - fx, y - fy) * unit >= line.h + (walk || shoulderOf(line))) return;
        const bank = line.plan ? line.plan.bank[v] + t * (line.plan.bank[v + 1] - line.plan.bank[v]) : 0;
        const z = line.z[v] + t * (line.z[v + 1] - line.z[v]) + bank * Math.max(-line.h, Math.min(line.h, left)) + (Math.abs(left) > line.h && walk ? KERB_RISE_M : 0);
        if (best === undefined || z > best) best = z;
    });
    return best;
}

/**
 * The stretches of a line, in metres along it, whose middle lies on a road near its heights. It samples each segment alone, from its
 * first vertex, so two tiles that hold a segment agree; a short gap between two stretches is none.
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
        const n = Math.ceil((metres[v + 1] - metres[v]) / STEP_M);
        for (let k = 0; k < n; k++) take(metres[v] + (k * (metres[v + 1] - metres[v])) / n);
    }
    take(metres[metres.length - 1]);
    if (from !== undefined) out.push([from, last]);
    return out.reduce<[number, number][]>((kept, stretch) => {
        const held = kept[kept.length - 1];
        if (held && stretch[0] - held[1] <= GAP_M) held[1] = stretch[1];
        else kept.push(stretch);
        return kept;
    }, []);
}

/** Lays the paths that lie on paved roads on those roads' surfaces, and notes the stretches in `ridden`. */
export function riders(lines: Line[]): void {
    const roads = roadsOf(lines);
    if (!roads.spans.length) return;
    const seen = new Map<string, number>(), name = (line: Line, v: number) => `${line.xy[2 * v]},${line.xy[2 * v + 1]}`;
    for (const line of lines) line.z.forEach((_, v) => seen.set(name(line, v), (seen.get(name(line, v)) ?? 0) + 1));
    for (const line of lines) {
        if (line.kind !== PATH || line.gallery || Object.keys(PROFILES)[line.profile] === 'steps' || line.z.length < 2) continue;
        const metres = alongOf(line), stretches = stretchesOf(line, metres, roads);
        if (!stretches.length) continue;
        // The next point past m where other heights hold: a shared vertex, another stretch or the line's end.
        const stops = metres.filter((_, v) => (seen.get(name(line, v)) ?? 0) > 1 || v === 0 || v === metres.length - 1);
        const room = (m: number, dir: 1 | -1) => Math.min(...[...stops, ...stretches.flat()].map(stop => (stop - m) * dir).filter(d => d > 1e-3), MOST_M);
        // Each end of a stretch takes the road's height, one step inside it; its ramp runs out to where the path's own heights take over.
        const ramps = stretches.flatMap(([a, b]) => ([[a, -1], [b, 1]] as const).map(([m, dir]) => {
            const p = pointAlong(line, metres, m), change = surfaceOn(roads, p.x, p.y)! - (line.z[p.v] + p.u * (line.z[p.v + 1] - line.z[p.v]));
            return { m, dir, change, over: Math.min(room(m, dir), Math.max(RETURN_LEAST_M, Math.abs(change) / RETURN_GRADE)) };
        }));
        cutLine(line, ramps.flatMap(({ m, dir, over }) => [m, m + dir * over]), metres);
        const after = alongOf(line), own = line.z.slice(), on = (m: number) => stretches.find(([a, b]) => m >= a - NEAR_M && m <= b + NEAR_M);
        const surface = own.map((_, v) => {
            const { x, y } = pointAlong(line, after, after[v]);
            return on(after[v]) ? surfaceOn(roads, x, y) : undefined;
        });
        line.z = own.map((z, v) => {
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
