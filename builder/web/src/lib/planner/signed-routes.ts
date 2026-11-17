import type { Coordinate } from './geo';
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
    profile_m?: (number | null)[];
    via?: number[];
    turnarounds?: number[];
    parent?: number;
    stage?: number;
    stages?: number[];
    start_udeg?: [number, number];
}

/** A record with a line, not a long route. */
export type RouteRecord = CatalogRecord & Required<Pick<CatalogRecord, 'line_udeg' | 'via'>>;

/** The catalog kinds that each bike rides. */
export const kinds: Record<BikeType, CatalogRecord['kind'][]> = {
    hiking: ['hiking', 'foot'], mtb: ['mtb'], road: ['bicycle'], gravel: ['bicycle'], touring: ['bicycle'],
};

/** The name of grade `index` of `grades_m`: T1 to T6 for hiking and foot, S0 to S5 for mountain bike. */
export const gradeName = (kind: 'hiking' | 'foot' | 'mtb', index: number) => kind === 'mtb' ? `S${index}` : `T${index + 1}`;

/** The plan of a route record: start, shaping points and finish, and the plan indices where it turns back on purpose. */
export function routePlan(route: RouteRecord): { points: Coordinate[]; turnarounds: number[] } {
    const line = decodeCoordinates(route.line_udeg);
    const via = route.via;
    return {
        points: [line[0], ...via.map(index => line[index]), line[line.length - 1]],
        // Vertex 0 is plan index 0 of a loop that turns back at its own start; a one-way route cannot turn back at its start.
        // The route request sends interior indices only.
        turnarounds: (route.turnarounds ?? []).filter(index => index > 0 || route.loop).map(index => index === 0 ? 0 : via.indexOf(index) + 1),
    };
}
