import { cumulative } from '../geo';
import { gradeThresholds, profileGrades } from '../grade-data';
import type { RoutingLine, Surface } from '../routing';

/** A run of the route with one fact, which search stretch questions read (`planner/search/README.md`). `from` and
 * `to` are kilometres on the line. A height run also has its height change in metres and its mean grade in percent, both
 * positive for a descent too. */
export interface RouteSegment { kind: string; from: number; to: number; ascent?: number; gradient?: number }

// The device's climb rule (`firmware/obc-route/src/climb.rs`), kept equal by `specs/vectors/climbs.json`: a climb gains
// at least 80 m over at least 400 m at a mean grade of 3 % or more, in whole metres and whole percent. It ends at its
// summit when the height falls 25 m below it or 300 m pass without a new one. Heights pass a 3 m dead band first.
const [CLIMB, , STEEP] = gradeThresholds;
const GAIN_M = 80, LENGTH_M = 400, DROP_M = 25, FLAT_M = 300, BAND_M = 3;
// Steep runs use the profile's 10 % band; its grades use a 100 m window, so a shorter run is window noise.
const SHORTEST_KM = .1;
/** The surfaces that search and the route options count as unpaved. */
export const UNPAVED: (Surface | null)[] = ['Gravel', 'Dirt', 'Rough'];

const known = new WeakMap<object, RouteSegment[]>();

/** The segments of a line, computed once for each line. Each segment merges adjacent edges of one kind. A manual leg has
 * no edge facts, so it gives no surface, pushing or closure segments; its heights can still give climbs. */
export function routeSegments(line: Pick<RoutingLine, 'coordinates' | 'elevation' | 'edges'>): RouteSegment[] {
    const cached = known.get(line);
    if (cached) return cached;
    const km = cumulative(line.coordinates), total = km.at(-1) ?? 0, heights = line.elevation;
    const { surfaces, pushing, closures } = line.edges;
    const plain = (kind: string, from: number, to: number): RouteSegment => ({ kind, from: round(km[from], 3), to: round(km[to], 3) });
    const height = (kind: string, from: number, to: number, change = (heights[to]! - heights[from]!)): RouteSegment =>
        ({ ...plain(kind, from, to), ascent: Math.round(change), gradient: round(change / (km[to] - km[from]) / 10, 1) });
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
        for (const { from, to, gain } of climbs(km, heights, sign)) segments.push(height(kind, from, to, gain));
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

/** The climbs (sign 1) or descents (sign -1) of the heights by the device rule, as point indices from trough to summit
 * and the gain in whole metres. A missing height resets the dead band and drops the open climb. */
export function climbs(km: readonly number[], heights: readonly (number | null)[], sign = 1): { from: number; to: number; gain: number }[] {
    const found: { from: number; to: number; gain: number }[] = [];
    let band: number | null = null, trough = -1, low = 0, summit = -1, high = 0;
    const close = () => {
        const length = (km[summit] - km[trough]) * 1000, gain = Math.trunc(high - low);
        if (length >= LENGTH_M && gain >= GAIN_M && Math.trunc(gain * 100 / Math.trunc(length)) >= CLIMB) found.push({ from: trough, to: summit, gain });
    };
    for (let i = 0; i < km.length; i++) {
        const raw = heights[i];
        if (raw === null) {
            band = null;
            trough = summit = -1;
            continue;
        }
        if (band === null || Math.abs(raw * sign - band) >= BAND_M) band = raw * sign;
        // A flat floor moves the trough along, so a climb starts where the rise starts.
        if (summit < 0) {
            if (trough < 0 || band <= low) [trough, low] = [i, band];
            else [summit, high] = [i, band];
        } else if (band > high) [summit, high] = [i, band];
        else if (high - band > DROP_M || (km[i] - km[summit]) * 1000 > FLAT_M) {
            close();
            [trough, low] = band < high ? [i, band] : [summit, high];
            summit = -1;
        }
    }
    if (summit >= 0) close();
    return found;
}

const round = (value: number, digits: number) => Math.round(value * 10 ** digits) / 10 ** digits;
