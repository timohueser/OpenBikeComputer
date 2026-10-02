import type { Coordinate } from './editor';
import { requestRoute, type EngineRoute, type RouteLeg, type RouteTotals, type Surface } from './routing';

/** One leg cut from a route answer. Its `elapsed` starts at zero. */
interface Leg extends Pick<RouteLeg, 'start' | 'end' | 'totals'> {
    package: string;
    geometry: Coordinate[];
    elevation: (number | null)[];
    elapsed: number[];
    surfaces: Surface[];
    pushing: boolean[];
}

function cut(route: EngineRoute, { from_index: from, to_index: to, start, end, totals }: RouteLeg): Leg {
    return {
        start, end, totals, package: route.package,
        geometry: route.geometry.slice(from, to + 1), elevation: route.elevation.slice(from, to + 1),
        elapsed: route.elapsed.slice(from, to + 1).map(seconds => seconds - route.elapsed[from]),
        surfaces: route.surfaces.slice(from, to), pushing: route.pushing.slice(from, to),
    };
}

/** A leg starts at the last point of the leg before it, as in one route answer. */
function stitch(legs: Leg[], profile: string): EngineRoute {
    const totals: RouteTotals = { distance_m: 0, ascent_m: 0, seconds: 0, surface_m: [0, 0, 0, 0, 0, 0], unknown_elevation_m: 0, pushing_m: 0 };
    // The id only tells the primary route from its alternatives; the server id needs the whole geometry.
    const route: EngineRoute = { id: 'primary', reason: 'primary', package: legs[0].package, profile, geometry: [], elevation: [], elapsed: [],
        surfaces: [], pushing: [], legs: [], snap_truncated: false, totals };
    for (const leg of legs) {
        const skip = route.geometry.length ? 1 : 0;
        const offset = route.elapsed.at(-1) ?? 0;
        const from = route.geometry.length - skip;
        route.geometry.push(...leg.geometry.slice(skip));
        route.elevation.push(...leg.elevation.slice(skip));
        route.elapsed.push(...leg.elapsed.slice(skip).map(seconds => seconds + offset));
        route.surfaces.push(...leg.surfaces);
        route.pushing.push(...leg.pushing);
        route.legs.push({ from_index: from, to_index: route.geometry.length - 1, start: leg.start, end: leg.end, totals: leg.totals });
        for (const key of ['distance_m', 'ascent_m', 'seconds', 'unknown_elevation_m', 'pushing_m'] as const) totals[key] += leg.totals[key];
        leg.totals.surface_m.forEach((metres, i) => totals.surface_m[i] += metres);
    }
    return route;
}

/** Routed legs by profile, end points and turnarounds, so an edit requests only the legs that it changed. */
export class LegCache {
    private legs = new Map<string, Leg>();
    private coordinates = 0;
    /** About ten 300 km trips. */
    constructor(private readonly limit = 200_000) {}

    /** The route through a run of routed points. It sends at most one request: for the legs from the first to the last
     * leg that is not cached. That request is pinned to the cached legs before and after it, so each join keeps its direction. */
    async route(points: Coordinate[], turnarounds: number[], profile: string, signal: AbortSignal): Promise<EngineRoute> {
        const turn = (index: number) => turnarounds.includes(index);
        // A turnaround joins legs in either direction, so it is never pinned.
        const joins = (a: Leg | undefined, b: Leg | undefined, at: number) => !a || !b || a.package === b.package && (turn(at) || a.end === b.start);
        const keys = points.slice(1).map((to, k) => JSON.stringify([profile, points[k], to, turn(k), turn(k + 1)]));
        const legs = keys.map(key => this.legs.get(key));
        for (let k = 1; k < legs.length; k++) if (!joins(legs[k - 1], legs[k], k)) legs[k] = undefined;
        const first = legs.findIndex(leg => !leg);
        if (first >= 0) {
            let last = legs.length - 1;
            while (legs[last]) last--;
            const before = legs[first - 1], after = legs[last + 1];
            const pins = { start_position: turn(first) ? undefined : before?.end, end_position: turn(last + 1) ? undefined : after?.start };
            const [answer] = await requestRoute(points.slice(first, last + 2), profile, signal, false,
                turnarounds.filter(t => t > first && t <= last).map(t => t - first), pins);
            const fresh = answer.legs.map(leg => cut(answer, leg));
            if (fresh.length !== last + 1 - first) throw new Error('The routing service returned an invalid response.');
            // A new routing package ignores the pins of the old one.
            if (!joins(before, fresh[0], first) || !joins(fresh.at(-1), after, last + 1)) {
                this.legs.clear();
                this.coordinates = 0;
                return this.route(points, turnarounds, profile, signal);
            }
            legs.splice(first, fresh.length, ...fresh);
        }
        legs.forEach((leg, k) => this.add(keys[k], leg!));
        return stitch(legs as Leg[], profile);
    }

    /** Keeps a leg as the most recent. The least recently used legs go when the cache holds too many coordinates. */
    private add(key: string, leg: Leg): void {
        const old = this.legs.get(key);
        if (old) this.coordinates -= old.geometry.length;
        this.legs.delete(key);
        this.legs.set(key, leg);
        this.coordinates += leg.geometry.length;
        for (const [oldest, { geometry }] of this.legs) {
            if (this.coordinates <= this.limit || oldest === key) break;
            this.legs.delete(oldest);
            this.coordinates -= geometry.length;
        }
    }
}
