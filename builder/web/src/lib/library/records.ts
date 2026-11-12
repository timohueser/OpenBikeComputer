/** The records of the library database, and the checks that a record read from a file or the database has their fields. */

export const bikes = ['road', 'gravel', 'touring', 'mtb', 'hiking'] as const;
export type Bike = typeof bikes[number];

/** The simplified line and the numbers of a route or a ride, so that a list or a map needs no other data. */
export interface Summary {
    /** Longitude, latitude, longitude, latitude, …, rounded to 5 decimals: the simplified line of `summarize`. */
    line: number[];
    /** The height in metres of each point of `line`, or null where it is not known. */
    heights: (number | null)[];
    /** West, south, east, north of the whole line, before it was simplified. */
    bounds: [number, number, number, number];
    lengthM: number;
    climbM: number;
    /** The highest point of the whole line, or null when no height is known. */
    highestM: number | null;
    seconds: number;
    bike: Bike;
}

/** `seconds` is the riding time that the routing gives. */
export interface RouteSummary extends Summary {
    days: number;
}

/** `seconds` is the whole time from the first point to the last. */
export interface RideSummary extends Summary {
    /** Unix seconds of the first point. */
    start: number;
    movingS: number;
}

export interface RouteRecord {
    id: string;
    revision: number;
    name: string;
    updatedAt: number;
    /** The id of the saved version that `trip` belongs to. */
    open?: string;
    /** Null only while the plan never had a calculated line. */
    summary: RouteSummary | null;
    /** The planner checks a plan when it opens it. */
    trip: unknown;
}

export interface RideRecord {
    /** The UTC time of the first point, `YYYYMMDDTHHMMSSZ`. Two files with the same key are the same ride. */
    key: string;
    name: string;
    source: 'device' | 'file';
    format: 'obcride' | 'fit' | 'gpx';
    summary: RideSummary;
    maxHr: number | null;
    ftp: number | null;
    group?: string;
    trip?: string;
    /** `storeId:objectId` of the ride on the device: a card gets a new `StoreId` when it is initialized, and it never
     * gives an `ObjectId` again. */
    device?: string;
}

export interface CoverRecord {
    /** The id of the route or the key of the ride. */
    id: string;
    state: string;
}

export interface DeletedRide {
    key: string;
    device?: string;
}

/** The values of the `state` store. */
export interface LibraryState {
    /** The route that the planner opens. */
    active: string | null;
    /** Routes that wait to go to the device. */
    waiting: string[];
    /** Routes to remove from the device. */
    leaving: string[];
    /** Rides that the rider deleted; no import adds them again. */
    deleted: DeletedRide[];
}

export const rideKey = /^\d{8}T\d{6}Z$/;

const isNumber = (value: unknown): value is number => typeof value === 'number' && Number.isFinite(value);
const isString = (value: unknown): value is string => typeof value === 'string';
const optional = (value: unknown, check: (value: unknown) => boolean) => value === undefined || check(value);
const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
const isStrings = (value: unknown): value is string[] => Array.isArray(value) && value.every(isString);

function isSummary(value: unknown): value is Summary {
    if (!isRecord(value)) return false;
    const { line, heights, bounds } = value;
    return Array.isArray(line) && line.length % 2 === 0 && line.every(isNumber)
        && Array.isArray(heights) && heights.length * 2 === line.length && heights.every(h => h === null || isNumber(h))
        && Array.isArray(bounds) && bounds.length === 4 && bounds.every(isNumber)
        && [value.lengthM, value.climbM, value.seconds].every(isNumber) && (value.highestM === null || isNumber(value.highestM))
        && bikes.includes(value.bike as Bike);
}

export const isRouteSummary = (value: unknown): value is RouteSummary =>
    isSummary(value) && Number.isInteger((value as RouteSummary).days) && (value as RouteSummary).days >= 1;

export const isRideSummary = (value: unknown): value is RideSummary =>
    isSummary(value) && isNumber((value as RideSummary).start) && isNumber((value as RideSummary).movingS);

export function isRouteRecord(value: unknown): value is RouteRecord {
    return isRecord(value) && isString(value.id) && !!value.id && Number.isInteger(value.revision) && (value.revision as number) >= 0
        && isString(value.name) && isNumber(value.updatedAt) && optional(value.open, isString)
        && (value.summary === null || isRouteSummary(value.summary)) && value.trip !== undefined;
}

export function isRideRecord(value: unknown): value is RideRecord {
    return isRecord(value) && isString(value.key) && rideKey.test(value.key) && isString(value.name)
        && ['device', 'file'].includes(value.source as string) && ['obcride', 'fit', 'gpx'].includes(value.format as string)
        && isRideSummary(value.summary) && (value.maxHr === null || isNumber(value.maxHr)) && (value.ftp === null || isNumber(value.ftp))
        && optional(value.group, isString) && optional(value.trip, isString) && optional(value.device, isString);
}

const isDeleted = (value: unknown): value is DeletedRide =>
    isRecord(value) && isString(value.key) && rideKey.test(value.key) && optional(value.device, isString);

/** The check of each value of the `state` store. */
export const stateChecks: { [K in keyof LibraryState]: (value: unknown) => value is LibraryState[K] } = {
    active: (value): value is string | null => value === null || isString(value),
    waiting: isStrings,
    leaving: isStrings,
    deleted: (value): value is DeletedRide[] => Array.isArray(value) && value.every(isDeleted),
};
