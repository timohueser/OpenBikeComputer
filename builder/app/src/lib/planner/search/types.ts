import type { Coordinate } from '../geo';
import type { RoutingLine } from '../routing';
import { SEARCH_URL } from './config';
import { searchLine } from './plan';
import { routeSegments } from './segments';

// Values of the query language are strings: the search contract (`apps/planner-search/query/contract.json`) lists them.
export type Quantity = { value: number; unit: string };
export type QueryDay = number | 'today' | 'tomorrow' | 'every';
export type Along = { ref: string; at?: Quantity; from?: Quantity; to?: Quantity };
export type QueryPoint = { name: string } | { kind: string } | { here: true } | { plan: 'start' | 'end' } | { day: QueryDay; part?: string } | { along: Along };
export type Where = { anchor?: Coordinate; scope?: string; day?: QueryDay; part?: string; near?: QueryPoint[]; along?: Along; before?: QueryPoint; after?: QueryPoint };
export interface QueryRequest {
    type: string;
    what?: string[] | string; cuisine?: string; name?: string; where?: Where; near?: QueryPoint;
    from?: QueryPoint; to?: QueryPoint; via?: QueryPoint[]; point?: QueryPoint; at?: QueryPoint;
    day?: QueryDay; days?: number; kind?: string; bike?: string; goal?: string;
    open?: { weekday?: string; day?: QueryDay; now?: true };
    radius?: Quantity; every?: Quantity; per_day?: Quantity; min?: Quantity;
    ignored?: string[];
}
export interface SearchContext {
    view: [number, number, number, number]; here?: Coordinate; startDate?: string; pointing?: Where;
    /** The full route: `km` and `seconds` hold one value per coordinate. `line` is the routed line of these coordinates. */
    plan: { coordinates: Coordinate[]; km: readonly number[]; seconds?: number[]; line?: RoutingLine; days: { number: number; from: number; to: number; rest: boolean }[];
        points: { id: string; label: string; coordinate: Coordinate; kind: string; placeKind?: string }[] };
}
/** The status at search time; `closesAt` is the local clock time of a closure within the next hour. */
export interface HoursStatus { state: 'open' | 'closed' | 'unknown'; closesAt?: string }
export interface SearchPlace {
    source: string; name: string; kind: string; lon: number; lat: number; city: string; region: string;
    distance: number; position?: { along: number; distance: number }; precision: 'place' | 'street' | 'house';
    opening_hours?: string; opening?: string; hoursStatus?: HoursStatus;
    website?: string; phone?: string; description?: string;
}
export interface ResolvedPoint { coordinate: Coordinate; label: string; source?: string; kind?: string; detail?: string; along?: number; alternatives?: ResolvedPoint[] }
export interface QueryChange {
    op: 'route' | 'end_day' | 'add_point' | 'remove_point' | 'split' | 'join' | 'reverse';
    point?: ResolvedPoint; points?: ResolvedPoint[]; day?: number; id?: string; kind?: string;
    range?: [number, number]; count?: number; boundaries?: number[]; bike?: string; goal?: string; days?: number; perDay?: Quantity;
}
export interface SearchAnswer {
    type: 'places' | 'change' | 'stretches' | 'unresolved'; request: QueryRequest;
    results?: SearchPlace[]; changes?: QueryChange[]; description?: string;
    stretches?: { from: number; to: number; label: string; coordinates: Coordinate[] }[];
    hasMore?: boolean; canRetry?: boolean; area?: string; note?: string; notice?: string; region?: string;
}
export type SearchState = { loading: boolean; error: string; answer: SearchAnswer | null };
export async function placeDetails(source: string, coordinate: Coordinate, region: string, signal: AbortSignal): Promise<SearchPlace | undefined> {
    const [lon, lat] = coordinate;
    const response = await fetch(`${SEARCH_URL}/query`, { method: 'POST', signal, headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ q: '', source, region, view: [lon - .01, lat - .01, lon + .01, lat + .01] }) });
    const result = await response.json();
    if (!response.ok) throw new Error(result.error || 'Place details are unavailable.');
    return result.results?.find((place: SearchPlace) => place.source === source);
}
/** The route is simplified and its segments are found here, at most once per route (see `searchLine` and `routeSegments`),
 * and never on a route change alone. */
export async function searchPlaces(q: string, context: SearchContext, region: string, limit: number, signal: AbortSignal, request?: QueryRequest): Promise<SearchAnswer> {
    const { coordinates, km, seconds, line, ...plan } = context.plan;
    const segments = line && routeSegments(line);
    const response = await fetch(`${SEARCH_URL}/query`, { method: 'POST', signal, headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ ...context, plan: { ...plan, ...searchLine(coordinates, km, seconds), segments }, q, region, limit, submitted: true, request }) });
    const result = await response.json();
    if (!response.ok) throw new Error(result.error || 'Search failed. Try again.');
    return result;
}
