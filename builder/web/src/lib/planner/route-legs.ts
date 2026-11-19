import type { Coordinate } from './geo';
import { appendEdges, MAX_REQUEST_POINTS, requestRoute, type Edges, type EngineRoute, type RouteFailure, type RouteLeg, type RouteTotals } from './routing';

/** One leg cut from a route answer, or a gap: the straight line of a leg with no legal path. Its `elapsed` starts at zero. */
interface Leg extends Pick<RouteLeg, 'start' | 'end' | 'totals' | 'gap'> {
    package: string;
    truncated: boolean;
    geometry: Coordinate[];
    elevation: (number | null)[];
    elapsed: number[];
    edges: Edges;
}

/** The legs `from` to `to` of one request, and the road positions that pin its ends. */
interface Span {
    from: number;
    to: number;
    pins: { start_position?: string; end_position?: string };
}

const noTotals = (): RouteTotals => ({ distance_m: 0, ascent_m: 0, seconds: 0, surface_m: [0, 0, 0, 0, 0, 0], unknown_elevation_m: 0, pushing_m: 0 });

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

/** A leg with no legal path from `from` to `to`. A request never asks for it again. */
const gapLeg = (from: Coordinate, to: Coordinate): Leg => ({ start: '', end: '', totals: noTotals(), gap: true, package: '', truncated: false,
    geometry: [from, to], elevation: [null, null], elapsed: [0, 0], edges: {} });

/** A leg starts at the last point of the leg before it, as in one route answer. A gap runs straight from the leg before it
 * to the leg after it. Its points have no height, so it adds no climb, and the leg after it keeps its own first point. */
function stitch(legs: Leg[], profile: string): EngineRoute {
    const totals = noTotals();
    // The id only tells the primary route from its alternatives; the server id needs the whole geometry.
    const route: EngineRoute = { id: 'primary', reason: 'primary', package: legs.find(leg => !leg.gap)?.package ?? '', profile, geometry: [],
        elevation: [], elapsed: [], edges: {}, legs: [], snap_truncated: false, totals };
    legs.forEach((leg, k) => {
        const geometry = leg.gap ? [route.geometry.at(-1) ?? leg.geometry[0], legs[k + 1]?.geometry[0] ?? leg.geometry[1]] : leg.geometry;
        const skip = route.geometry.length && !(legs[k - 1]?.gap && !leg.gap) ? 1 : 0;
        const offset = route.elapsed.at(-1) ?? 0;
        const from = route.geometry.length - skip;
        // The join from a gap to the leg after it has no length.
        if (route.geometry.length && !skip) appendEdges(route.edges, from - 1, undefined, 0, 1);
        appendEdges(route.edges, from, leg.edges, 0, geometry.length - 1);
        // A loop, not a spread: a long leg exceeds the argument limit of some engines.
        for (let i = skip; i < geometry.length; i++) {
            route.geometry.push(geometry[i]);
            route.elevation.push(leg.elevation[i]);
            route.elapsed.push(leg.elapsed[i] + offset);
        }
        route.snap_truncated ||= leg.truncated;
        route.legs.push({ from_index: from, to_index: route.geometry.length - 1, start: leg.start, end: leg.end, totals: leg.totals, ...leg.gap && { gap: true } });
        for (const key of ['distance_m', 'ascent_m', 'seconds', 'unknown_elevation_m', 'pushing_m'] as const) totals[key] += leg.totals[key];
        leg.totals.surface_m.forEach((metres, i) => totals.surface_m[i] += metres);
    });
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

    /** The route through a run of routed points. The legs that are not cached go out in `chunks`, at most two requests at a
     * time. The first chunk of each run of missing legs is pinned to the cached leg before it, and the last chunk to the
     * cached leg after it, so each such join keeps its direction; a gap pins nothing. Each answer is kept as it arrives, so
     * a failed or aborted route asks only for the rest. Then each leg that does not join the leg before it goes out again,
     * pinned to both neighbours. A leg with no path under a pin goes out once more one leg wider on that side, so that its
     * point can pass the other way; with no leg in the error, on each pinned side. An error names its point or leg by its
     * index in `points`. */
    async route(points: Coordinate[], turnarounds: number[], profile: string, signal: AbortSignal, restarted = false): Promise<EngineRoute> {
        const turn = (index: number) => turnarounds.includes(index);
        // A turnaround joins legs in either direction, and a gap joins any leg.
        const joins = (a: Leg | undefined, b: Leg | undefined, at: number) => !a || !b || a.gap || b.gap || a.package === b.package && (turn(at) || a.end === b.start);
        const pin = (leg: Leg | undefined) => leg?.gap ? undefined : leg;
        const keys = legKeys(points, turnarounds, profile);
        const legs = keys.map(key => this.legs.get(key));
        // The legs that went out wider, and the legs that went out again to join their neighbours.
        const widened = new Set<number>(), joined = new Set<number>();
        for (let round = 0; ; round++) {
            for (let k = 1; k < legs.length; k++) {
                if (joins(legs[k - 1], legs[k], k)) continue;
                // A new routing package ignores the positions of the old one.
                if (legs[k - 1]!.package !== legs[k]!.package) {
                    if (restarted) throw new Error('The routing service returned an invalid response.');
                    this.legs.clear();
                    this.coordinates = 0;
                    return this.route(points, turnarounds, profile, signal, true);
                }
                // A leg pinned to both neighbours joins them, unless the service ignores its pins.
                if (joined.has(k)) throw new Error('The routing service returned an invalid response.');
                if (round) joined.add(k);
                legs[k] = undefined;
            }
            const spans: Span[] = [];
            for (let first = legs.findIndex(leg => !leg), last = first; first >= 0; first = legs.findIndex((leg, k) => k > last && !leg)) {
                last = first;
                while (last + 1 < legs.length && !legs[last + 1]) last++;
                // A turnaround stays inside the request: only an interior turnaround may depart on the other road.
                while (turn(first) && !legs[first - 1]?.gap) first--;
                while (turn(last + 1) && !legs[last + 1]?.gap) last++;
                const gap = chunks(Math.max(first, (spans.at(-1)?.to ?? -1) + 1), last, turnarounds);
                spans.push(...gap.map(([from, to], c) => ({ from, to, pins: { start_position: c ? undefined : pin(legs[from - 1])?.end,
                    end_position: c < gap.length - 1 ? undefined : pin(legs[to + 1])?.start } })));
            }
            if (!spans.length) break;
            // After a failure no request starts; the one in flight ends and is kept.
            let next = 0;
            const failures: (Span & { error: unknown })[] = [];
            const send = async () => {
                while (!failures.length && next < spans.length) {
                    const span = spans[next++];
                    try {
                        const fresh = await this.request(points, turnarounds, profile, signal, span);
                        legs.splice(span.from, fresh.length, ...fresh);
                        this.keep(keys.slice(span.from, span.to + 1), fresh, keys);
                    } catch (error) {
                        failures.push({ ...span, error });
                    }
                }
            };
            await Promise.all([send(), send()]);
            if (!failures.length) continue;
            const { error, from, to, pins } = failures[0];
            const { code, point, leg } = error as RouteFailure;
            // A service that names no leg can mean either end.
            const failed = leg === undefined ? [from, to] : [from + leg];
            // Only a missing path under a pin can change with a wider request; a busy service must not get a larger one.
            const wider = code !== 'no_path' || [...widened].some(k => from <= k && k <= to) ? []
                : [failed.includes(from) && pins.start_position ? from - 1 : -1, failed.includes(to) && pins.end_position ? to + 1 : -1].filter(k => k >= 0);
            if (!wider.length) {
                throw Object.assign(error as object, point === undefined ? {} : { point: from + point }, leg === undefined ? {} : { leg: from + leg });
            }
            failed.forEach(k => widened.add(k));
            wider.forEach(k => legs[k] = undefined);
        }
        this.keep(keys, legs as Leg[]);
        return stitch(legs as Leg[], profile);
    }

    /** Keeps leg `k` of `points` as a gap, so a route through it asks no request for it until one of its points moves. */
    gap(points: Coordinate[], turnarounds: number[], profile: string, k: number): void {
        const keys = legKeys(points, turnarounds, profile);
        this.keep([keys[k]], [gapLeg(points[k], points[k + 1])], keys);
    }

    /** The legs of `span`, from one request. */
    private async request(points: Coordinate[], turnarounds: number[], profile: string, signal: AbortSignal, { from, to, pins }: Span): Promise<Leg[]> {
        for (let attempt = 0; ; attempt++) {
            signal.throwIfAborted();
            try {
                const [answer] = await requestRoute(points.slice(from, to + 2), profile, signal, false,
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

    /** Keeps the legs of `route`, the answer for `points` without turnarounds, so a run through those points needs no request. */
    add(points: Coordinate[], route: EngineRoute): void {
        this.keep(legKeys(points, [], route.profile), route.legs.map(leg => cut(route, leg)));
    }

    /** Keeps the legs of a route as the most recent. When the cache holds too many coordinates, the least recently used
     * legs of other routes than `route` go. */
    private keep(keys: string[], legs: Leg[], route = keys): void {
        keys.forEach((key, k) => {
            this.coordinates -= this.legs.get(key)?.geometry.length ?? 0;
            this.legs.delete(key);
            this.legs.set(key, legs[k]);
            this.coordinates += legs[k].geometry.length;
        });
        const current = new Set(route);
        for (const [key, { geometry }] of this.legs) {
            if (this.coordinates <= this.limit) break;
            if (current.has(key)) continue;
            this.legs.delete(key);
            this.coordinates -= geometry.length;
        }
    }
}
