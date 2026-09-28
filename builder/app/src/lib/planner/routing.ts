import { cumulative, orderedRoutePoints, routingKey, type Coordinate, type Trip } from './editor';

export type Surface = 'Unknown' | 'Paved' | 'Compacted' | 'Gravel' | 'Dirt' | 'Rough';

export interface RouteTotals {
    distance_m: number;
    ascent_m: number;
    descent_m: number;
    seconds: number;
    surface_m: number[];
    unknown_elevation_m: number;
    pushing_m: number;
}
export interface EngineRoute {
    id: string;
    reason: string;
    package: string;
    profile: string;
    geometry: Coordinate[];
    elevation: (number | null)[];
    elapsed: number[];
    surfaces: Surface[];
    totals: RouteTotals;
    legs: { from_index: number; to_index: number; totals: RouteTotals }[];
    snap_truncated: boolean;
}
export interface RoutingLine {
    choiceId: string;
    key: string;
    coordinates: Coordinate[];
    elevation: (number | null)[];
    elapsed: number[];
    surfaces: Surface[];
    stops: { id: string; distance: number }[];
    seconds: number;
    alternatives: EngineRoute[];
    alternativesReady: boolean;
    profile: string;
    unknownSurfaceKm: number;
    pushingKm: number;
    unroutedKm: number;
}

const endpoint = import.meta.env.VITE_PLANNER_ROUTING_URL ?? '/routing';

export async function requestRoute(points: Coordinate[], profile: string, signal: AbortSignal, alternatives = false, turnarounds: number[] = []): Promise<EngineRoute[]> {
    const response = await fetch(`${endpoint}/v1/route`, {
        method: 'POST', headers: { 'Content-Type': 'application/json' }, signal,
        body: JSON.stringify({ points, profile, alternatives, turnarounds }),
    });
    const data = await response.json().catch(() => { throw new Error('The routing service returned an invalid response.'); });
    if (!response.ok) throw new Error(data.message ?? 'Routing is unavailable.');
    if (!Array.isArray(data.routes) || !data.routes.length) throw new Error('The routing service returned no route.');
    return data.routes;
}

export function profileId(trip: Trip): string {
    const variant = ({ Shorter: 'shorter', Smoother: 'smoother', 'Less climbing': 'less-climbing' } as Record<string, string>)[trip.preset ?? ''];
    return `${trip.bike ?? 'touring'}${variant ? `/${variant}` : ''}`;
}

export function selectRoute(trip: Trip, route: EngineRoute, alternatives: EngineRoute[]): RoutingLine {
    const points = orderedRoutePoints(trip);
    const distance = cumulative(route.geometry);
    return {
        choiceId: route.id, key: routingKey(trip), coordinates: route.geometry, elevation: route.elevation, elapsed: route.elapsed, surfaces: route.surfaces, seconds: route.totals.seconds,
        profile: route.profile, alternatives, alternativesReady: true, unknownSurfaceKm: route.totals.surface_m[0] / 1000, pushingKm: route.totals.pushing_m / 1000, unroutedKm: 0,
        stops: [{ id: points[0].id, distance: 0 }, ...route.legs.map((leg, i) => ({ id: points[i + 1].id, distance: distance[leg.to_index] }))],
    };
}

/** Consecutive routed legs form one request, so a shaping point keeps its road direction. */
export async function calculateLine(trip: Trip, signal: AbortSignal, alternatives = false): Promise<RoutingLine> {
    const points = orderedRoutePoints(trip);
    if (points.length < 2) throw new Error('Choose a start and finish to calculate a route.');
    const result: RoutingLine = { choiceId: '', key: routingKey(trip), coordinates: [], elevation: [], elapsed: [], surfaces: [], stops: [], seconds: 0,
        alternatives: [], alternativesReady: true, profile: profileId(trip), unknownSurfaceKm: 0, pushingKm: 0, unroutedKm: 0 };
    let distance = 0;
    function append(line: Coordinate[], elevation: (number | null)[], elapsed: number[], surfaces: Surface[]) {
        const last = result.coordinates.at(-1);
        const gap = last ? cumulative([last, line[0]])[1] : 0;
        // A join to a manually drawn leg is itself an explicit, unverified connector.
        if (gap > 0) { result.unroutedKm += gap; result.unknownSurfaceKm += gap; result.seconds += gap / 15 * 3600; distance += gap; }
        const offset = last && gap === 0 ? 1 : 0;
        for (let i = offset; i < line.length; i++) {
            if (result.coordinates.length) result.surfaces.push(i === 0 ? 'Unknown' : surfaces[i - 1] ?? 'Unknown');
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
            append(line, line.map(() => null), lengths.map(km => km / 15 * 3600), []);
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
            }
        }
        const canOfferAlternatives = i === 1 && until === points.length - 1 && !turnarounds.length;
        const routes = await requestRoute(expanded, result.profile, signal, alternatives && canOfferAlternatives, turnarounds);
        const route = routes[0];
        if (canOfferAlternatives) { result.alternatives = routes; result.choiceId = route.id; result.alternativesReady = alternatives; }
        const start = append(route.geometry, route.elevation, route.elapsed, route.surfaces);
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
    const i = lengths.findIndex(d => d >= distance);
    if (i < 0) return line.seconds;
    if (i === 0) return 0;
    const share = (distance - lengths[i - 1]) / (lengths[i] - lengths[i - 1] || 1);
    return line.elapsed[i - 1] + share * (line.elapsed[i] - line.elapsed[i - 1]);
}
