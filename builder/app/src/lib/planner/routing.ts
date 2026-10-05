import { presetSuffix, ridingProfiles } from './riding-profiles';
import { orderedRoutePoints, type DrawnCoordinate, type Trip } from './editor';
import { cumulative, firstIndex, type Coordinate } from './geo';
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
    /** A corridor alternative's shaping point: a request through the start, this point and the finish gives the same line. */
    via?: Coordinate;
}
/** The calculated line of a plan. It is not part of the plan: the planner calculates it again from the plan's points. */
export interface RoutingLine {
    choiceId: string;
    coordinates: Coordinate[];
    elevation: (number | null)[];
    elapsed: number[];
    edges: Edges;
    stops: { id: string; distance: number }[];
    seconds: number;
    /** The primary route, then with `alternativesReady` its alternatives. A line that is not one routed run through all
     * points of its plan has no alternatives and is ready. */
    alternatives: EngineRoute[];
    alternativesReady: boolean;
    profile: string;
    unknownSurfaceKm: number;
    pushingKm: number;
    unroutedKm: number;
    /** Ridden kilometres whose ascent is unknown; a transfer is not ridden, so it never counts. */
    unknownElevationKm: number;
    /** Routing package of the routed legs. */
    package?: string;
}

const endpoint = import.meta.env.VITE_PLANNER_ROUTING_URL ?? '/routing';
/** Longer than the deadlines of the route service, so its own error arrives first. A hung call frees its caller. */
const timeoutMs = { '/v1/route': 20_000, '/v1/shape': 40_000 };

/** The answer, or an error with a message for the rider and the service's error `code`. A caller's abort passes through. */
async function post(path: keyof typeof timeoutMs, body: unknown, signal?: AbortSignal): Promise<unknown> {
    const timeout = AbortSignal.timeout(timeoutMs[path]);
    let response: Response | undefined, data: { code?: string; message?: string } | undefined;
    try {
        response = await fetch(`${endpoint}${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json' },
            signal: signal ? AbortSignal.any([signal, timeout]) : timeout, body: JSON.stringify(body) });
        data = await response.json();
    } catch (error) {
        if (signal?.aborted) throw error;
        // A proxy answers with an HTML page while the service restarts.
        throw new Error(timeout.aborted ? 'The routing service did not answer in time. Retry shortly.'
            : !response ? 'The routing service is unreachable. Check your connection and retry.'
            : response.ok ? 'The routing service returned an invalid response.' : 'Routing is unavailable. Retry shortly.');
    }
    if (!response.ok) throw Object.assign(new Error(data?.message ?? 'Routing is unavailable. Retry shortly.'), { code: data?.code });
    return data;
}

/** With `'only'`, the answer leaves out the primary route and can be empty. */
export async function requestRoute(points: Coordinate[], profile: string, signal: AbortSignal, alternatives: boolean | 'only' = false, turnarounds: number[] = [],
    pins: { start_position?: string; end_position?: string } = {}): Promise<EngineRoute[]> {
    const body = { points, profile, ...(alternatives === 'only' ? { alternatives_only: true } : { alternatives }), turnarounds, ...pins };
    const routes = decodeRoutes(await post('/v1/route', body, signal) as Parameters<typeof decodeRoutes>[0]);
    if (!routes.length && alternatives !== 'only') throw new Error('The routing service returned no route.');
    return routes;
}

/** Points that `/v1/route` routes along `line` (`specs/route-api.md`); a turnaround is an index into `points`. */
export interface Shape {
    points: Coordinate[];
    turnarounds: number[];
}

/** A failed or hung call keeps the file's line. */
export async function requestShape(line: Coordinate[], profile: string): Promise<Shape> {
    const data = await post('/v1/shape', { line, profile }) as Partial<Shape>;
    const points = data.points, turnarounds = data.turnarounds ?? [];
    if (!Array.isArray(points) || points.length < 2 || !points.every(p => Array.isArray(p) && p.length === 2 && p.every(Number.isFinite))
        || !Array.isArray(turnarounds) || !turnarounds.every(i => Number.isInteger(i) && i > 0 && i < points.length - 1))
        throw new Error('The routing service returned an invalid response.');
    return { points, turnarounds };
}

/** The primary route and the alternatives of a line whose alternatives are not ready. */
export async function requestAlternatives(trip: Trip, line: RoutingLine, signal: AbortSignal): Promise<EngineRoute[]> {
    const points = orderedRoutePoints(trip).map(point => point.coordinate);
    return [...line.alternatives, ...await requestRoute(points, line.profile, signal, 'only')];
}

export function profileId(trip: Trip): string {
    const variant = presetSuffix(trip.preset);
    return `${trip.bike ?? 'touring'}${variant ? `/${variant}` : ''}`;
}

const keys = new WeakMap<Trip, string>();

/** What the line of a trip depends on, so equal keys share one line. Itinerary edits and labels never change it. */
export function routingKey(trip: Trip): string {
    let key = keys.get(trip);
    if (key === undefined) {
        key = JSON.stringify([trip.bike ?? 'touring', trip.preset ?? 'Balanced', orderedRoutePoints(trip).map(p => [p.id, p.coordinate, p.leg ?? 'routed', p.drawn ?? [], p.kind === 'detour', p.anchor, p.turnaround])]);
        keys.set(trip, key);
    }
    return key;
}

/** Consecutive routed legs form one run, so a shaping point keeps its road direction. */
export async function calculateLine(trip: Trip, signal: AbortSignal, legs: LegCache): Promise<RoutingLine> {
    const points = orderedRoutePoints(trip);
    if (points.length < 2) throw new Error('Choose a start and finish to calculate a route.');
    const result: RoutingLine = { choiceId: '', coordinates: [], elevation: [], elapsed: [], edges: {}, stops: [], seconds: 0,
        alternatives: [], alternativesReady: true, profile: profileId(trip), unknownSurfaceKm: 0, pushingKm: 0, unroutedKm: 0, unknownElevationKm: 0 };
    // Seconds per kilometre on a manual leg or connector.
    const pace = 3600 / ridingProfiles[trip.bike ?? 'touring'].kmh;
    let distance = 0;
    let afterTransfer = false;
    function append(line: Coordinate[], elevation: (number | null)[], elapsed: number[], edges: Edges, transfer = false) {
        const last = result.coordinates.at(-1);
        const gap = last ? cumulative([last, line[0]])[1] : 0;
        // A join to a manually drawn leg is itself an explicit, unverified connector. A join to a transfer is not ridden.
        if (gap > 0 && !transfer && !afterTransfer) {
            result.unroutedKm += gap; result.unknownSurfaceKm += gap; result.seconds += gap * pace;
            if (result.elevation.at(-1) === null || elevation[0] === null) result.unknownElevationKm += gap;
        }
        distance += gap;
        // After a transfer the next leg repeats its first vertex, so no segment joins the heights at the two ends of the transfer.
        const offset = last && gap === 0 && !afterTransfer ? 1 : 0;
        const connector = last && !offset ? 1 : 0;
        afterTransfer = transfer;
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
        if (end.leg && end.leg !== 'routed') {
            const drawn: DrawnCoordinate[] = [origin, ...(end.leg === 'drawn' ? end.drawn ?? [] : []), end.coordinate];
            const line = drawn.map((c): Coordinate => [c[0], c[1]]);
            const heights = drawn.map(c => c[2] ?? null);
            // A drawn leg repeats its points with their heights; the points themselves have none.
            const same = (a: number, b: number) => line[a][0] === line[b][0] && line[a][1] === line[b][1];
            if (line.length > 2 && same(0, 1)) heights[0] ??= heights[1];
            if (line.length > 2 && same(line.length - 1, line.length - 2)) heights[line.length - 1] ??= heights[line.length - 2];
            const lengths = cumulative(line);
            const ridden = end.leg !== 'transfer';
            append(line, heights, lengths.map(km => ridden ? km * pace : 0), {}, !ridden);
            const km = ridden ? lengths.at(-1)! : 0;
            result.unknownSurfaceKm += km;
            result.unroutedKm += km;
            if (ridden) heights.forEach((h, k) => { if (k && (h === null || heights[k - 1] === null)) result.unknownElevationKm += lengths[k] - lengths[k - 1]; });
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
        result.unknownElevationKm += route.totals.unknown_elevation_m / 1000;
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
