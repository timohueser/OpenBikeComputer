import { presetSuffix } from './riding-profiles';
import { cumulative, firstIndex, orderedRoutePoints, routingKey, type Coordinate, type Trip } from './editor';
import { decodeRoutes } from './route-answer';
import type { LegCache } from './route-legs';

export type Surface = 'Unknown' | 'Paved' | 'Compacted' | 'Gravel' | 'Dirt' | 'Rough';

export interface RouteTotals {
    distance_m: number;
    ascent_m: number;
    seconds: number;
    surface_m: number[];
    unknown_elevation_m: number;
    pushing_m: number;
}
/** `start` and `end` are opaque road positions (`specs/route-api.md`). */
export interface RouteLeg {
    from_index: number;
    to_index: number;
    start: string;
    end: string;
    totals: RouteTotals;
}
/** Why the rider may have no access to an edge that the route uses; `specs/route-api.md` lists the kinds. */
export interface RouteClosure {
    kind: 'permit' | 'private' | 'farm' | 'sidepath' | 'discouraged' | 'limited' | 'seasonal' | 'conditional' | 'unclear';
    condition: string;
}
/** The facts of each edge, one channel for each fact (`specs/route-api.md`). A missing channel is null on every edge, and
 * a manual section, whose facts are not verified, is null in every channel. A channel that the planner does not know
 * travels with the others. */
export interface Edges {
    surfaces?: (Surface | null)[];
    pushing?: (boolean | null)[];
    /** The possible closures of each edge. */
    closures?: (RouteClosure[] | null)[];
    /** The SAC hiking grade of each edge: 0 (`strolling`) or 1 (T1) to 6 (T6). */
    sac_scale?: (number | null)[];
    /** The MTB grade of each edge: 0 (S0) to 6 (S6). */
    mtb_scale?: (number | null)[];
}

/** Appends the edges `from` to `to` of `source`, or null edges without a source, to `target`, which holds `length` edges.
 * A channel that one side lacks is null there. */
export function appendEdges(target: Edges, length: number, source: Edges | undefined, from: number, to: number): void {
    const into = target as Record<string, unknown[]>, values = (source ?? {}) as Record<string, unknown[]>;
    for (const channel of new Set([...Object.keys(into), ...Object.keys(values)])) {
        const edges = into[channel] ??= Array(length).fill(null);
        for (let i = from; i < to; i++) edges.push(values[channel]?.[i] ?? null);
    }
}

export interface EngineRoute {
    id: string;
    reason: string;
    package: string;
    profile: string;
    geometry: Coordinate[];
    elevation: (number | null)[];
    elapsed: number[];
    edges: Edges;
    totals: RouteTotals;
    legs: RouteLeg[];
    snap_truncated: boolean;
}
export interface RoutingLine {
    choiceId: string;
    key: string;
    coordinates: Coordinate[];
    elevation: (number | null)[];
    elapsed: number[];
    edges: Edges;
    stops: { id: string; distance: number }[];
    seconds: number;
    alternatives: EngineRoute[];
    alternativesReady: boolean;
    profile: string;
    unknownSurfaceKm: number;
    pushingKm: number;
    unroutedKm: number;
    /** Routing package of the routed legs. */
    package?: string;
    /** A picked alternative with the profile of its primary route, such as another corridor. No request for its plan returns it. */
    picked?: boolean;
}

const endpoint = import.meta.env.VITE_PLANNER_ROUTING_URL ?? '/routing';

/** With `'only'`, the answer leaves out the primary route and can be empty. */
export async function requestRoute(points: Coordinate[], profile: string, signal: AbortSignal, alternatives: boolean | 'only' = false, turnarounds: number[] = [],
    pins: { start_position?: string; end_position?: string } = {}): Promise<EngineRoute[]> {
    const response = await fetch(`${endpoint}/v1/route`, {
        method: 'POST', headers: { 'Content-Type': 'application/json' }, signal,
        body: JSON.stringify({ points, profile, ...(alternatives === 'only' ? { alternatives_only: true } : { alternatives }), turnarounds, ...pins }),
    });
    const data = await response.json().catch(() => { throw new Error('The routing service returned an invalid response.'); });
    if (!response.ok) throw Object.assign(new Error(data.message ?? 'Routing is unavailable.'), { code: data.code as string | undefined });
    const routes = decodeRoutes(data);
    if (!routes.length && alternatives !== 'only') throw new Error('The routing service returned no route.');
    return routes;
}

/** The primary route and the alternatives of a line whose alternatives are not ready. Such a line is one routed request
 * through the points of its plan. It holds its primary route, unless it was stored without its routes (`storedPlan`). */
export async function requestAlternatives(trip: Trip, line: RoutingLine, signal: AbortSignal): Promise<EngineRoute[]> {
    const points = orderedRoutePoints(trip).map(point => point.coordinate);
    return [...line.alternatives, ...await requestRoute(points, line.profile, signal, line.alternatives.length ? 'only' : true)];
}

export function profileId(trip: Trip): string {
    const variant = presetSuffix(trip.preset);
    return `${trip.bike ?? 'touring'}${variant ? `/${variant}` : ''}`;
}

export function selectRoute(trip: Trip, route: EngineRoute, alternatives: EngineRoute[]): RoutingLine {
    const points = orderedRoutePoints(trip);
    const distance = cumulative(route.geometry);
    const primary = alternatives[0] ?? route;
    return {
        choiceId: route.id, key: routingKey(trip), coordinates: route.geometry, elevation: route.elevation, elapsed: route.elapsed, edges: route.edges, seconds: route.totals.seconds,
        profile: route.profile, package: route.package, alternatives, alternativesReady: true, unknownSurfaceKm: route.totals.surface_m[0] / 1000, pushingKm: route.totals.pushing_m / 1000, unroutedKm: 0,
        stops: [{ id: points[0].id, distance: 0 }, ...route.legs.map((leg, i) => ({ id: points[i + 1].id, distance: distance[leg.to_index] }))],
        picked: route.id !== primary.id && route.profile === primary.profile,
    };
}

/** Consecutive routed legs form one run, so a shaping point keeps its road direction. */
export async function calculateLine(trip: Trip, signal: AbortSignal, legs: LegCache): Promise<RoutingLine> {
    const points = orderedRoutePoints(trip);
    if (points.length < 2) throw new Error('Choose a start and finish to calculate a route.');
    const result: RoutingLine = { choiceId: '', key: routingKey(trip), coordinates: [], elevation: [], elapsed: [], edges: {}, stops: [], seconds: 0,
        alternatives: [], alternativesReady: true, profile: profileId(trip), unknownSurfaceKm: 0, pushingKm: 0, unroutedKm: 0 };
    let distance = 0;
    function append(line: Coordinate[], elevation: (number | null)[], elapsed: number[], edges: Edges) {
        const last = result.coordinates.at(-1);
        const gap = last ? cumulative([last, line[0]])[1] : 0;
        // A join to a manually drawn leg is itself an explicit, unverified connector.
        if (gap > 0) { result.unroutedKm += gap; result.unknownSurfaceKm += gap; result.seconds += gap / 15 * 3600; distance += gap; }
        const offset = last && gap === 0 ? 1 : 0;
        const connector = gap > 0 ? 1 : 0;
        const length = Math.max(result.coordinates.length - 1, 0);
        appendEdges(result.edges, length, undefined, 0, connector);
        appendEdges(result.edges, length + connector, edges, 0, line.length - 1);
        for (let i = offset; i < line.length; i++) {
            result.coordinates.push(line[i]);
            result.elevation.push(elevation[i]);
            result.elapsed.push(elapsed[i] + result.seconds);
        }
        result.seconds += elapsed.at(-1) ?? 0;
        const start = distance;
        distance += cumulative(line).at(-1)!;
        return start;
    }
    result.stops.push({ id: points[0].id, distance: 0 });
    for (let i = 1; i < points.length;) {
        const end = points[i];
        const before = points[i - 1];
        const origin = before.kind === 'detour' ? before.anchor ?? before.coordinate : before.coordinate;
        if (end.leg === 'straight' || end.leg === 'drawn') {
            const line = [origin, ...(end.leg === 'drawn' ? end.drawn ?? [] : []), end.coordinate];
            const lengths = cumulative(line);
            append(line, line.map(() => null), lengths.map(km => km / 15 * 3600), {});
            const km = lengths.at(-1)!;
            result.unknownSurfaceKm += km;
            result.unroutedKm += km;
            result.stops.push({ id: end.id, distance });
            i++;
            continue;
        }
        let until = i;
        while (until + 1 < points.length && (points[until + 1].leg ?? 'routed') === 'routed') until++;
        const expanded: Coordinate[] = [origin];
        const turnarounds: number[] = [];
        const ends = new Map<number, string>();
        for (let p = i; p <= until; p++) {
            const point = points[p];
            if (point.kind === 'detour') {
                if (!point.anchor) throw new Error('Choose a return point on the route for this visit.');
                expanded.push(point.anchor, point.coordinate, point.anchor);
                turnarounds.push(expanded.length - 2);
                ends.set(expanded.length - 3, point.id);
            } else {
                expanded.push(point.coordinate);
                ends.set(expanded.length - 2, point.id);
                if (point.turnaround && p < until) turnarounds.push(expanded.length - 1);
            }
        }
        const canOfferAlternatives = i === 1 && until === points.length - 1 && !turnarounds.length;
        const route = await legs.route(expanded, turnarounds, result.profile, signal);
        result.package = route.package;
        if (canOfferAlternatives) { result.alternatives = [route]; result.choiceId = route.id; result.alternativesReady = false; }
        const start = append(route.geometry, route.elevation, route.elapsed, route.edges);
        const lengths = cumulative(route.geometry);
        result.unknownSurfaceKm += route.totals.surface_m[0] / 1000;
        result.pushingKm += route.totals.pushing_m / 1000;
        for (let leg = 0; leg < route.legs.length; leg++) {
            const id = ends.get(leg);
            if (id) result.stops.push({ id, distance: start + lengths[route.legs[leg].to_index] });
        }
        i = until + 1;
    }
    return result;
}

export function movingSecondsAt(line: RoutingLine, progress: number): number {
    const lengths = cumulative(line.coordinates);
    const distance = progress * (lengths.at(-1) ?? 0);
    const i = firstIndex(lengths.length, index => lengths[index] >= distance);
    if (i === lengths.length) return line.seconds;
    if (i === 0) return 0;
    const share = (distance - lengths[i - 1]) / (lengths[i] - lengths[i - 1] || 1);
    return line.elapsed[i - 1] + share * (line.elapsed[i] - line.elapsed[i - 1]);
}
