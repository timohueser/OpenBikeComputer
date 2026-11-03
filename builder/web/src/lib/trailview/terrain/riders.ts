// A path that OSM maps along or across a paved road, inside its core: it is the road's sidewalk or a crossing. The bake holds
// its heights apart from the road's, so it would run under the road's faces and vanish. As far as its middle lies on the road,
// it lies on the road's surface, and past each end of that stretch it returns to its own heights. Where its whole core lies
// on the road, it is inside, as a guest is between its junction and its stop: the road draws there.
import { alongOf, cutLine, GUEST_INSIDE, pointAlong, returnTo } from './junction-plan';
import { KERB_RISE_M, metresPerUnit, PATH, PROFILES, ROAD, SHOULDER_M, SURFACES, walkOf, type Line } from './bins';
import { gridOf, LEFT, near, RIGHT } from './town';

/** The step of the search along a path; a gap between two stretches this short is none. */
const STEP_M = 0.25, GAP_M = 6;
/** A way this far off a road's surface passes it, on a level of its own. */
const APART_M = 4;
/** A return takes at most this many metres. */
const MOST_M = 16;

const PAVED = ['main', 'road', 'oneway', 'paving'];
const surfaceOf = (line: Line) => (Object.keys(SURFACES) as (keyof typeof SURFACES)[])[(line.code & 15) - 1];
const shoulderOf = (line: Line) => SHOULDER_M[surfaceOf(line)] ?? 0;

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
        // A segment holds only the points whose foot lies on it: so a path that runs on from a road's end, or past the end of
        // a short last segment, stays off the road.
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

/** Whether the middle of a line `at` metres along it, or with `core` all its core, lies on a road near its heights; never where a junction's plan holds it inside. */
function onRoad(line: Line, metres: number[], roads: ReturnType<typeof roadsOf>, at: number, core: boolean): boolean {
    const p = pointAlong(line, metres, at), own = line.z[p.v] + p.u * (line.z[p.v + 1] - line.z[p.v]);
    if (line.inside?.[p.v]) return false;
    const dx = line.xy[2 * p.v + 2] - line.xy[2 * p.v], dy = line.xy[2 * p.v + 3] - line.xy[2 * p.v + 1], across = line.h / metresPerUnit(p.y) / (Math.hypot(dx, dy) || 1);
    return (core ? [0, 1, -1] : [0]).every(a => {
        const z = surfaceOn(roads, p.x + dy * across * a, p.y - dx * across * a);
        return z !== undefined && Math.abs(z - own) < APART_M;
    });
}

/**
 * The stretches of a line, in metres along it, where `on` holds. It samples each segment alone, from its first vertex, so
 * two tiles that hold a segment agree; a gap of up to `gap` metres between two stretches is none. A single point, such as a vertex that
 * the path shares with a road's end, is no stretch.
 */
function stretchesOf(metres: number[], on: (at: number) => boolean, gap = GAP_M): [number, number][] {
    const out: [number, number][] = [];
    let from: number | undefined, last = 0;
    const take = (at: number) => {
        if (on(at)) {
            from ??= at;
            last = at;
        } else if (from !== undefined) {
            out.push([from, last]);
            from = undefined;
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
        if (held && stretch[0] - held[1] <= gap) held[1] = stretch[1];
        else kept.push(stretch);
        return kept;
    }, []).filter(([a, b]) => b > a);
}

/**
 * Lays each stretch of a path whose middle lies on a paved road on the road's surface, and returns it to its own heights
 * past each end. Where its whole core lies on the road, it is inside.
 */
export function riders(lines: Line[]): void {
    const roads = roadsOf(lines);
    if (!roads.spans.length) return;
    const seen = new Map<string, number>(), name = (line: Line, v: number) => `${line.xy[2 * v]},${line.xy[2 * v + 1]}`;
    for (const line of lines) line.z.forEach((_, v) => seen.set(name(line, v), (seen.get(name(line, v)) ?? 0) + 1));
    for (const line of lines) {
        if (line.kind !== PATH || line.gallery || Object.keys(PROFILES)[line.profile] === 'steps' || line.z.length < 2) continue;
        const metres = alongOf(line), stretches = stretchesOf(metres, at => onRoad(line, metres, roads, at, false));
        if (!stretches.length) continue;
        const inside = stretchesOf(metres, at => onRoad(line, metres, roads, at, true), 0);
        // Past an end of a stretch, other heights hold at a shared vertex or another stretch. A line's end says nothing: the tile may cut it there.
        const stops = [...metres.filter((_, v) => (seen.get(name(line, v)) ?? 0) > 1), ...stretches.flat()];
        const room = (m: number, dir: 1 | -1) => Math.min(...stops.map(stop => (stop - m) * dir).filter(d => d > 1e-3), MOST_M);
        const surface = (m: number, along = alongOf(line)) => {
            const p = pointAlong(line, along, m);
            return surfaceOn(roads, p.x, p.y);
        };
        const ends = stretches.flatMap(([a, b]) => ([[a, -1], [b, 1]] as const).map(([m, dir]) => ({ m, dir, z: surface(m, metres)!, room: room(m, dir) })));
        cutLine(line, [...stretches.flat(), ...inside.flat()], metres);
        for (const { m, dir, z: from, room: most } of ends) returnTo(line, m, dir, from, most);
        const after = alongOf(line), on = (m: number, list: [number, number][]) => list.some(([a, b]) => m > a - 1e-3 && m < b + 1e-3);
        // On a stretch the path lies on the road's surface; where a vertex finds none, its height runs on from its neighbours.
        const z = after.map(m => (on(m, stretches) ? surface(m, after) : undefined));
        line.z = line.z.map((own, v) => {
            if (!on(after[v], stretches)) return own;
            if (z[v] !== undefined) return z[v]!;
            let [p, q] = [v, v];
            while (p > 0 && z[p] === undefined && on(after[p - 1], stretches)) p--;
            while (q + 1 < z.length && z[q] === undefined && on(after[q + 1], stretches)) q++;
            return mix(z[p] ?? own, z[q] ?? own, (after[v] - after[p]) / ((after[q] - after[p]) || 1));
        });
        if (line.plan) for (let v = 1; v + 1 < line.z.length; v++) if (on(after[v], stretches)) line.plan.grade[v] = (line.z[v + 1] - line.z[v - 1]) / (after[v + 1] - after[v - 1] || 1);
        line.inside ??= new Array(line.z.length - 1).fill(0);
        for (let s = 0; s + 1 < after.length; s++) if (on((after[s] + after[s + 1]) / 2, inside)) line.inside[s] = GUEST_INSIDE;
    }
}

const mix = (a: number, b: number, t: number) => a + (b - a) * t;
