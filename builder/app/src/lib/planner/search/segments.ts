import { cumulative } from '../geo';
import { gradeThresholds, profileGrades } from '../grade-data';
import type { RoutingLine, Surface } from '../routing';

/** A run of the route with one fact, which search stretch questions read (`planner/search/README.md`). `from` and
 * `to` are kilometres on the line. A height run also has its height change in metres and its mean grade in percent, both
 * positive for a descent too. */
export interface RouteSegment { kind: string; from: number; to: number; ascent?: number; gradient?: number }

// The device's climb rule (`firmware/obc-route/src/climb.rs`): a climb gains at least 80 m over at least 400 m at a
// mean grade of 3 % or more. It ends at its summit when the height falls 25 m below it or 300 m pass without a new one.
const [CLIMB, , STEEP] = gradeThresholds;
const GAIN_M = 80, LENGTH_KM = .4, DROP_M = 25, FLAT_KM = .3;
// Steep runs use the profile's 10 % band; its grades use a 100 m window, so a shorter run is window noise.
const SHORTEST_KM = .1;
// The surfaces that the alternatives list counts as unpaved.
const UNPAVED: (Surface | null)[] = ['Gravel', 'Dirt', 'Rough'];

const known = new WeakMap<object, RouteSegment[]>();

/** The segments of a line, computed once for each line. Each segment merges adjacent edges of one kind. A manual leg has
 * no edge facts, so it gives no surface, pushing or closure segments; its heights can still give climbs. */
export function routeSegments(line: Pick<RoutingLine, 'coordinates' | 'elevation' | 'edges'>): RouteSegment[] {
    const cached = known.get(line);
    if (cached) return cached;
    const km = cumulative(line.coordinates), total = km.at(-1) ?? 0, heights = line.elevation;
    const { surfaces, pushing, closures } = line.edges;
    const plain = (kind: string, from: number, to: number): RouteSegment => ({ kind, from: round(km[from], 3), to: round(km[to], 3) });
    const height = (kind: string, from: number, to: number, sign = 1): RouteSegment => {
        const change = (heights[to]! - heights[from]!) * sign;
        return { ...plain(kind, from, to), ascent: Math.round(change), gradient: round(change / (km[to] - km[from]) / 10, 1) };
    };
    /** The runs of adjacent edges that `keep` accepts, as point indices. */
    const runs = (keep: (edge: number) => boolean) => {
        const found: [number, number][] = [];
        for (let from = 0; from < km.length - 1; from++) {
            if (!keep(from)) continue;
            let to = from + 1;
            while (to < km.length - 1 && keep(to)) to++;
            found.push([from, to]);
            from = to;
        }
        return found;
    };
    const segments: RouteSegment[] = [];
    for (const [kind, sign] of [['climb', 1], ['descent', -1]] as const)
        for (const [from, to] of climbs(km, heights, sign)) segments.push(height(kind, from, to, sign));
    const grades = profileGrades(heights.map((h, i) => ({ progress: km[i] / (total || 1), height: h })), total);
    for (const [from, to] of runs(i => (grades[i] ?? 0) >= STEEP))
        if (km[to] - km[from] >= SHORTEST_KM) segments.push(height('steep', from, to));
    // A routed edge always has a pushing value, and its missing surface is unknown, as in the route summary.
    const surface = (i: number) => pushing?.[i] == null ? null : surfaces?.[i] ?? 'Unknown';
    const facts: [string, (edge: number) => boolean][] = [
        ['unpaved', i => UNPAVED.includes(surface(i))], ['unknown_surface', i => surface(i) === 'Unknown'],
        ['pushing', i => pushing?.[i] === true], ['closure', i => !!closures?.[i]?.length],
    ];
    for (const [kind, keep] of facts) for (const [from, to] of runs(keep)) segments.push(plain(kind, from, to));
    known.set(line, segments);
    return segments;
}

/** The climbs (sign 1) or descents (sign -1) of the heights, as [trough, summit] point indices. A missing height drops
 * the open climb, as on the device. */
function climbs(km: readonly number[], heights: (number | null)[], sign: number): [number, number][] {
    const found: [number, number][] = [], h = (i: number) => heights[i]! * sign;
    let trough = -1, summit = -1;
    const close = () => {
        const gain = h(summit) - h(trough), length = km[summit] - km[trough];
        if (gain >= GAIN_M && length >= LENGTH_KM && gain / length / 10 >= CLIMB) found.push([trough, summit]);
    };
    for (let i = 0; i < km.length; i++) {
        if (heights[i] === null) trough = summit = -1;
        else if (summit < 0) {
            if (trough < 0 || h(i) < h(trough)) trough = i;
            else if (h(i) > h(trough)) summit = i;
        } else if (h(i) > h(summit)) summit = i;
        else if (h(summit) - h(i) > DROP_M || km[i] - km[summit] > FLAT_KM) {
            close();
            trough = h(i) < h(summit) ? i : summit;
            summit = -1;
        }
    }
    if (summit >= 0) close();
    return found;
}

const round = (value: number, digits: number) => Math.round(value * 10 ** digits) / 10 ** digits;
