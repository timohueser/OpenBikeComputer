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
    /** The moving time of the plan route at the default pace, in seconds; not in every release. */
    moving_s?: number;
    cells: string[];
    line_udeg?: number[];
    profile_m?: (number | null)[];
    via?: number[];
    turnarounds?: number[];
    parent?: number;
    stage?: number;
    stages?: number[];
    start_udeg?: [number, number];
    /** Long record, region file only. */
    parts?: TrailPart[];
    /** Long record, region file only. */
    trail_profile_m?: (number | null)[];
    stage_count?: number;
    trail?: number;
    trail_stage?: number;
    /** Section of a cut line only: its number in `trail`. */
    section?: number;
    /** The credit of a second route source, which each view that shows the record shows. */
    credit?: string;
}

/** A part of a long record, in trail order. */
export interface TrailPart {
    id: number;
    stage?: number;
    name?: string;
    length_m?: number;
    ascent_m?: number;
    /** As on the route record of the part; not in every release. */
    moving_s?: number;
    hardest?: number;
    end_m: number;
    cells?: string[];
    missing?: true;
    cut?: true;
    start_udeg?: [number, number];
    end_udeg?: [number, number];
    backwards?: true;
    jump?: true;
}

/** A trail of a far file (`specs/route-catalog.md`, "Trail files"). */
export type TrailEntry = Pick<CatalogRecord, 'id' | 'kind' | 'name' | 'ref' | 'symbol' | 'rank' | 'loop' | 'length_m' | 'ascent_m' | 'hardest' | 'stage_count'> & {
    /** Pieces of the far line, each encoded as `line_udeg`. */
    far_udeg: number[][];
};

/** A place of a facts file (`specs/route-catalog.md`, "Facts files"). Positions are `[longitude, latitude]` in microdegrees. */
export interface FactsPlace {
    kind: 'parking' | 'lift' | 'peak' | 'hut' | 'lake' | 'waterfall';
    name?: string;
    ele_m?: number;
    /** The node; the top station of a lift; the point of an area nearest to the route. */
    at_udeg: [number, number];
    size?: 'large' | 'small';
    valley_udeg?: [number, number];
    /** Lift only: the place id of the nearest car park within 300 m of the valley station. */
    parking?: string;
}

/** A place id and the straight-line distance in metres between the place and the nearest point of the route. */
export type NearPlace = [place: string, metres: number];

/** The entry of a route record in a facts file. Shares are whole percents of its samples. A list with no item is absent.
 * (`RouteFacts` of `route-facts.ts` is the facts of a planned trip.) */
export interface CatalogFacts {
    parking?: NearPlace[];
    lifts?: NearPlace[];
    places?: NearPlace[];
    paved_pct: number;
    big_road_pct: number;
    water_pct: number;
    canopy_pct?: number;
    /** A lift whose valley station is at one end of the route and whose top station is at the other, and the end at the valley. */
    lift_ends?: [lift: string, valley: 'start' | 'end'];
    /** The moving time in the other direction, in seconds. */
    back_s?: number;
    /** The first and the last day step of the longest run of steps on which the route is usually free of snow; `first` is
     * greater than `last` for a run across 1 September. Empty with no free step; absent with no data. */
    snow_free?: [first: number, last: number] | [];
}

/** A facts file of one grid cell. `routes` maps a record `id`, as a decimal string, to its entry. */
export interface FactsFile {
    format: 1;
    places: Record<string, FactsPlace>;
    routes: Record<string, CatalogFacts>;
}

/** A record with a line, not a long route. */
export type RouteRecord = CatalogRecord & Required<Pick<CatalogRecord, 'line_udeg' | 'via'>>;

/** The catalog kinds that each bike rides. */
export const kinds: Record<BikeType, CatalogRecord['kind'][]> = {
    hiking: ['hiking', 'foot'], mtb: ['mtb'], road: ['bicycle'], gravel: ['bicycle'], touring: ['bicycle'],
};

/** The distinct credits of `records`, in their order. A long route joins the credits of its parts with `; `. */
export const recordCredits = (records: { credit?: string }[]) =>
    [...new Set(records.flatMap(record => record.credit?.split('; ').filter(Boolean) ?? []))];

/** The name of grade `index` of `grades_m`: T1 to T6 for hiking and foot, S0 to S5 for mountain bike. */
export const gradeName = (kind: 'hiking' | 'foot' | 'mtb', index: number) => kind === 'mtb' ? `S${index}` : `T${index + 1}`;

/**
 * The id of the long trail that `record` is in: for a part, its long route L (`trail ?? parent`) or, when L is a cut stage,
 * the trail of L; for any other record, its own id. Undefined when L is not loaded: each file that holds a part also holds
 * L, so only a broken file gives it.
 */
export function longTrailId(record: CatalogRecord, recordOf: (id: number) => CatalogRecord | undefined): number | undefined {
    const longId = record.trail ?? record.parent;
    if (longId === undefined) return record.id;
    const long = recordOf(longId);
    return long && (long.trail ?? long.id);
}

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
