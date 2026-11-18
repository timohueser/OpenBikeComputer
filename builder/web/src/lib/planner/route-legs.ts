import type { Coordinate } from './geo';
import { appendEdges, MAX_REQUEST_POINTS, requestRoute, type Edges, type EngineRoute, type RouteLeg, type RouteTotals } from './routing';

/** One leg cut from a route answer. Its `elapsed` starts at zero. */
interface Leg extends Pick<RouteLeg, 'start' | 'end' | 'totals'> {
    package: string;
    truncated: boolean;
    geometry: Coordinate[];
    elevation: (number | null)[];
    elapsed: number[];
    edges: Edges;
}

function cut(route: EngineRoute, { from_index: from, to_index: to, start, end, totals }: RouteLeg): Leg {
    const edges: Edges = {};
    appendEdges(edges, 0, route.edges, from, to);
    return {
        start, end, totals, package: route.package, truncated: route.snap_truncated,
        geometry: route.geometry.slice(from, to + 1), elevation: route.elevation.slice(from, to + 1),
        elapsed: route.elapsed.slice(from, to + 1).map(seconds => seconds - route.elapsed[from]),
        edges,
    };
}

/** A leg starts at the last point of the leg before it, as in one route answer. */
function stitch(legs: Leg[], profile: string): EngineRoute {
    const totals: RouteTotals = { distance_m: 0, ascent_m: 0, seconds: 0, surface_m: [0, 0, 0, 0, 0, 0], unknown_elevation_m: 0, pushing_m: 0 };
    const route: EngineRoute = { package: legs[0].package, profile, geometry: [], elevation: [], elapsed: [],
        edges: {}, legs: [], snap_truncated: false, totals };
    for (const leg of legs) {
        const skip = route.geometry.length ? 1 : 0;
        const offset = route.elapsed.at(-1) ?? 0;
        const from = route.geometry.length - skip;
        appendEdges(route.edges, from, leg.edges, 0, leg.geometry.length - 1);
        // A loop, not a spread: a long leg exceeds the argument limit of some engines.
        for (let i = skip; i < leg.geometry.length; i++) {
            route.geometry.push(leg.geometry[i]);
            route.elevation.push(leg.elevation[i]);
            route.elapsed.push(leg.elapsed[i] + offset);
        }
        route.snap_truncated ||= leg.truncated;
        route.legs.push({ from_index: from, to_index: route.geometry.length - 1, start: leg.start, end: leg.end, totals: leg.totals });
        for (const key of ['distance_m', 'ascent_m', 'seconds', 'unknown_elevation_m', 'pushing_m'] as const) totals[key] += leg.totals[key];
        leg.totals.surface_m.forEach((metres, i) => totals.surface_m[i] += metres);
    }
    return route;
}

const legKeys = (points: Coordinate[], turnarounds: number[], profile: string) =>
    points.slice(1).map((to, k) => JSON.stringify([profile, points[k], to, turnarounds.includes(k), turnarounds.includes(k + 1)]));

/** The route API asks to send `busy` and `internal` again later. */
const RETRIES = 2, RETRY_MS = 1000;

/** The legs `first` to `last` as consecutive leg ranges `[from, to]` of at most one request each. Each range starts at the
 * end point of the range before, and no range ends at a turnaround: only an interior turnaround may depart on the other road. */
export function chunks(first: number, last: number, turnarounds: number[]): [number, number][] {
    const spans: [number, number][] = [];
    for (let from = first; from <= last;) {
        let to = Math.min(from + MAX_REQUEST_POINTS - 2, last);
        while (to < last && to > from && turnarounds.includes(to + 1)) to--;
        spans.push([from, to]);
        from = to + 1;
    }
    return spans;
}

/** Routed legs by profile, end points and turnarounds, so an edit requests only the legs that it changed. */
export class LegCache {
    private legs = new Map<string, Leg>();
    private coordinates = 0;
    /** About ten 300 km trips. */
    constructor(private readonly limit = 200_000) {}

    /** The route through a run of routed points. Each gap of legs that are not cached goes out by itself, in `chunks`, one
     * after the other. Each chunk is pinned to the leg before it, and the last chunk of a gap to the cached leg after it, so
     * each join keeps its direction. Each chunk is kept as it arrives, so a failed or aborted run asks only for the rest.
     * When a pinned chunk finds no path, it goes out again with one more leg on each side, so that a neighbour point can
     * pass the other way. When that finds no path too, a run with cached legs goes out whole; a run without fails. */
    async route(points: Coordinate[], turnarounds: number[], profile: string, signal: AbortSignal, whole = false): Promise<EngineRoute> {
        const turn = (index: number) => turnarounds.includes(index);
        // A turnaround joins legs in either direction.
        const joins = (a: Leg | undefined, b: Leg | undefined, at: number) => !a || !b || a.package === b.package && (turn(at) || a.end === b.start);
        const keys = legKeys(points, turnarounds, profile);
        const legs = keys.map(key => whole ? undefined : this.legs.get(key));
        for (let k = 1; k < legs.length; k++) if (!joins(legs[k - 1], legs[k], k)) legs[k] = undefined;
        const cached = legs.some(Boolean);
        // The first leg of the chunk that went out again with one more leg on each side.
        let widened: number | undefined;
        for (let first = legs.findIndex(leg => !leg); first >= 0; first = legs.findIndex(leg => !leg)) {
            let last = first;
            while (last + 1 < legs.length && !legs[last + 1]) last++;
            // A turnaround stays inside the request: only an interior turnaround may depart on the other road.
            while (turn(first)) first--;
            while (turn(last + 1)) last++;
            const after = legs[last + 1];
            let span: [number, number] = [first, last];
            try {
                for (const [from, to] of chunks(first, last, turnarounds)) {
                    span = [from, to];
                    const fresh = await this.request(points, turnarounds, profile, signal, from, to,
                        { start_position: legs[from - 1]?.end, end_position: to === last ? after?.start : undefined });
                    // A new routing package ignores the pins of the old one.
                    if (!joins(legs[from - 1], fresh[0], from) || (to === last && !joins(fresh.at(-1), after, last + 1))) {
                        this.legs.clear();
                        this.coordinates = 0;
                        return this.route(points, turnarounds, profile, signal);
                    }
                    legs.splice(from, fresh.length, ...fresh);
                    this.keep(keys.slice(from, to + 1), fresh);
                }
            } catch (error) {
                // Only a missing path under a pin can change with a wider request; a busy service must not get a larger one.
                const [from, to] = span;
                if (!(legs[from - 1] || (to === last && after)) || (error as { code?: string }).code !== 'no_path') throw error;
                if (widened !== undefined && from <= widened && widened <= to) {
                    if (!cached) throw error;
                    return this.route(points, turnarounds, profile, signal, true);
                }
                widened = from;
                legs.fill(undefined, Math.max(from - 1, 0), to + 2);
            }
        }
        this.keep(keys, legs as Leg[]);
        return stitch(legs as Leg[], profile);
    }

    /** The legs `from` to `to` of `points`, from one request. */
    private async request(points: Coordinate[], turnarounds: number[], profile: string, signal: AbortSignal, from: number, to: number,
        pins: { start_position?: string; end_position?: string }): Promise<Leg[]> {
        for (let attempt = 0; ; attempt++) {
            signal.throwIfAborted();
            try {
                const answer = await requestRoute(points.slice(from, to + 2), profile, signal,
                    turnarounds.filter(t => t > from && t <= to).map(t => t - from), pins);
                const fresh = answer.legs.map(leg => cut(answer, leg));
                if (fresh.length !== to + 1 - from) throw new Error('The routing service returned an invalid response.');
                return fresh;
            } catch (error) {
                if (attempt === RETRIES || !['busy', 'internal'].includes((error as { code?: string }).code ?? '')) throw error;
                await new Promise(resolve => setTimeout(resolve, RETRY_MS));
            }
        }
    }

    /** Keeps the legs of a route as the most recent. When the cache holds too many coordinates, the least recently used
     * legs of other routes go. */
    private keep(keys: string[], legs: Leg[]): void {
        keys.forEach((key, k) => {
            this.coordinates -= this.legs.get(key)?.geometry.length ?? 0;
            this.legs.delete(key);
            this.legs.set(key, legs[k]);
            this.coordinates += legs[k].geometry.length;
        });
        const current = new Set(keys);
        for (const [key, { geometry }] of this.legs) {
            if (this.coordinates <= this.limit) break;
            if (current.has(key)) continue;
            this.legs.delete(key);
            this.coordinates -= geometry.length;
        }
    }
}
