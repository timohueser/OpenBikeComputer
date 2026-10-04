import type { Coordinate } from './editor';
import { corridorTiles, routeDistance } from './place-index';
import type { BikeType } from './riding-profiles';
import { decodeCoordinates } from './route-answer';

/** One record of a catalog cell file, as `specs/route-catalog.md` specifies. */
export interface CatalogRecord {
    id: number;
    kind: 'hiking' | 'foot' | 'bicycle' | 'mtb';
    name?: string;
    ref?: string;
    operator?: string;
    description?: string;
    website?: string;
    symbol?: string;
    rank: number;
    loop: boolean;
    length_m: number;
    ascent_m: number;
    descent_m: number;
    grades_m?: number[];
    hardest?: number;
    cells: string[];
    line_udeg?: number[];
    via?: number[];
    turnarounds?: number[];
    parent?: number;
    stage?: number;
    stages?: number[];
    start_udeg?: [number, number];
}

/** A record with a line, not a long route. */
export type RouteRecord = CatalogRecord & Required<Pick<CatalogRecord, 'line_udeg' | 'via'>>;

export type RouteShape = 'any' | 'loop' | 'one-way';
export type RouteSort = 'nearest' | 'shortest' | 'longest' | 'most-climb' | 'least-climb';
/** Inclusive bounds. An absent bound is no limit. */
export interface Bounds { from?: number; to?: number }

export interface RouteQuery {
    start: Coordinate;
    radiusKm: number;
    activity: BikeType;
    shape: RouteShape;
    distanceKm?: Bounds;
    climbM?: Bounds;
    /** Grade indices of the hardest part, 0 to 3: T1–T4 for hiking, S0–S3 for mountain bike. Other activities ignore it. */
    hardest?: [number, number];
    sort: RouteSort;
    /** Offline: the downloaded cells. Only routes that lie wholly inside them match. */
    covered?: ReadonlySet<string>;
}

export interface RouteMatch { route: CatalogRecord; distanceM: number }

/** A cell file, or null when the cell is not covered. */
export type CellLoader = (id: string) => Promise<CatalogRecord[] | null>;

const kinds: Record<BikeType, CatalogRecord['kind'][]> = {
    hiking: ['hiking', 'foot'], mtb: ['mtb'], road: ['bicycle'], gravel: ['bicycle'], touring: ['bicycle'],
};
const sortKeys: Record<RouteSort, (match: RouteMatch) => number> = {
    nearest: match => match.distanceM,
    shortest: match => match.route.length_m,
    longest: match => -match.route.length_m,
    'most-climb': match => -match.route.ascent_m,
    'least-climb': match => match.route.ascent_m,
};

const within = (value: number, { from, to }: Bounds = {}, scale = 1) =>
    (from ?? -Infinity) * scale <= value && value <= (to ?? Infinity) * scale;

function passes(route: CatalogRecord, query: RouteQuery): boolean {
    const { covered, shape, hardest } = query;
    if (!kinds[query.activity].includes(route.kind)) return false;
    if (covered && !route.cells.every(cell => covered.has(cell))) return false;
    if (shape !== 'any' && route.loop !== (shape === 'loop')) return false;
    if (!within(route.length_m, query.distanceKm, 1000) || !within(route.ascent_m, query.climbM)) return false;
    if (!hardest || (query.activity !== 'hiking' && query.activity !== 'mtb')) return true;
    // An ungraded route is easy for the upper bound, but only an explicit grade meets a lower bound.
    const [min, max] = hardest;
    return (route.hardest ?? 0) <= max && (min === 0 || (route.hardest ?? -1) >= min);
}

/**
 * The routes within the radius that pass the filters, in the sort order with ties by id. The distance is from the
 * start to the nearest point of the route; a long route measures to its own start. A search at a larger radius holds
 * every match of a smaller one, so the matches of one search at the largest radius give the count for each smaller radius.
 */
export async function searchRoutes(query: RouteQuery, loadCell: CellLoader): Promise<RouteMatch[]> {
    const { start, radiusKm, covered } = query;
    const cells = corridorTiles([start], radiusKm, 9).map(key => key.replaceAll('/', '-')).filter(id => !covered || covered.has(id));
    const records = new Map<number, CatalogRecord>();
    for (const file of await Promise.all(cells.map(loadCell))) file?.forEach(route => records.set(route.id, route));
    const matches: RouteMatch[] = [];
    for (const route of records.values()) {
        if (!passes(route, query)) continue;
        const km = routeDistance(decodeCoordinates(route.start_udeg ?? route.line_udeg ?? []), radiusKm)(start);
        if (km !== Infinity) matches.push({ route, distanceM: km * 1000 });
    }
    const key = sortKeys[query.sort];
    return matches.sort((a, b) => key(a) - key(b) || a.route.id - b.route.id);
}

/** The plan of a route record: start, shaping points and finish, and the plan indices where it turns back on purpose. */
export function routePlan(route: RouteRecord): { points: Coordinate[]; turnarounds: number[] } {
    const line = decodeCoordinates(route.line_udeg);
    const via = route.via;
    return {
        points: [line[0], ...via.map(index => line[index]), line[line.length - 1]],
        // A loop can turn back at its own start, which is no interior point of this plan.
        turnarounds: (route.turnarounds ?? []).filter(index => via.includes(index)).map(index => via.indexOf(index) + 1),
    };
}

/** Index of the line vertex nearest to `place`, the first on a tie: where a loop plan starts. */
export function nearestVertex(line: Coordinate[], place: Coordinate): number {
    const kx = Math.cos(place[1] * Math.PI / 180);
    const squared = ([lon, lat]: Coordinate) => ((lon - place[0]) * kx) ** 2 + (lat - place[1]) ** 2;
    return line.reduce((best, vertex, index) => squared(vertex) < squared(line[best]) ? index : best, 0);
}
