import type { Coordinate } from '../editor';
import { SEARCH_URL } from './config';

export type Quantity = { value: number; unit: 'km' | 'h' | 'm' | '%' };
export type QueryDay = number | 'today' | 'tomorrow' | 'every';
export type Along = { ref: 'km' | 'start' | 'end' | 'here'; at?: Quantity; from?: Quantity; to?: Quantity };
export type QueryPoint = { name: string } | { kind: string } | { here: true } | { plan: 'start' | 'end' } | { day: QueryDay; part?: 'start' | 'middle' | 'end' } | { along: Along };
export type Where = { anchor?: Coordinate; scope?: 'route' | 'view' | 'here'; day?: QueryDay; part?: 'start' | 'middle' | 'end'; near?: QueryPoint[]; along?: Along; before?: QueryPoint; after?: QueryPoint };
export interface QueryRequest {
    type: 'places' | 'place' | 'route' | 'stretches' | 'end_day' | 'add_point' | 'remove_point' | 'split' | 'join' | 'reverse' | 'reroute' | 'none';
    what?: string[] | string; cuisine?: 'pizza' | 'kebab'; name?: string; where?: Where; near?: QueryPoint;
    from?: QueryPoint; to?: QueryPoint; via?: QueryPoint[]; point?: QueryPoint; at?: QueryPoint;
    day?: QueryDay; days?: number; kind?: string; bike?: string; goal?: string;
    open?: { weekday?: string; day?: QueryDay; now?: true };
    radius?: Quantity; every?: Quantity; per_day?: Quantity; min?: Quantity;
    ignored?: string[];
}
export interface SearchContext {
    view: [number, number, number, number]; here?: Coordinate; startDate?: string; pointing?: Where;
    plan: { coordinates: Coordinate[]; km: number[]; days: { number: number; from: number; to: number; rest: boolean }[];
        points: { id: string; label: string; coordinate: Coordinate; kind: string; placeKind?: string }[]; hours?: number[] };
}
export interface HoursStatus { state: 'open' | 'closed' | 'unknown'; checkedAt: number; validUntil: number; closesAt?: number }
export interface SearchPlace {
    source: string; name: string; kind: string; lon: number; lat: number; city: string; region: string;
    distance: number; position?: { along: number; distance: number }; precision: 'place' | 'street' | 'house';
    opening_hours?: string; opening?: string; hoursStatus?: HoursStatus;
}
export interface ResolvedPoint { coordinate: Coordinate; label: string; source?: string; kind?: string; detail?: string; along?: number; alternatives?: ResolvedPoint[] }
export interface QueryChange {
    op: 'route' | 'end_day' | 'add_point' | 'remove_point' | 'split' | 'join' | 'reverse' | 'reroute';
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
export async function searchPlaces(q: string, context: SearchContext, region: string, limit: number, signal: AbortSignal, request?: QueryRequest): Promise<SearchAnswer> {
    const response = await fetch(`${SEARCH_URL}/query`, { method: 'POST', signal, headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ ...context, q, region, limit, submitted: true, request }) });
    const result = await response.json();
    if (!response.ok) throw new Error(result.error || 'Search failed. Try again.');
    return result;
}
