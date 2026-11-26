import type { MapPoint } from './map-types';

/** The week in focus of a long trip: 7 days. The day list, the elevation profile and the night search show only these
 * days. */
export interface Week {
    /** The first and the last day. */
    start: number;
    end: number;
    /** The nights to search: the days inside and the night before the week. */
    nights: number[];
}

const length = 7;
/** A trip with fewer days shows all its days, with no week. */
export const weekFrom = 15;

/** The first day of a week in a trip of `days` days. */
const clamp = (days: number, start: number) => Math.max(1, Math.min(days - length + 1, start));

/** The week that starts at day `start`, held inside a trip of `days` days; null for a trip of fewer than `weekFrom` days. */
export function weekOf(days: number, start: number): Week | null {
    if (days < weekFrom) return null;
    const first = clamp(days, start), before = Math.max(1, first - 1);
    return { start: first, end: first + length - 1, nights: Array.from({ length: first + length - before }, (_, i) => before + i) };
}

/** The moves of the week in a trip of `days` days; each gives the new first day. */
export const centredOn = (days: number, day: number) => clamp(days, day - 3);
export const step = (days: number, start: number, by: number) => clamp(days, start + by);
/** The least move that puts day `day` inside the week. */
export const containing = (days: number, start: number, day: number) => clamp(days, Math.min(day, Math.max(start, day - length + 1)));
export const home = 1;
export const end = (days: number) => clamp(days, days);

const inside = (week: Week, day: number) => day >= week.start && day <= week.end;

/** How the map draws a day: in its colour inside the week, quiet outside it, and lit under the pointer on the strip. */
export function dayLook(week: Week | null, lit: number | null, day: number): 'colour' | 'quiet' | 'lit' {
    return day === lit ? 'lit' : !week || inside(week, day) ? 'colour' : 'quiet';
}

const shaping = new Set<MapPoint['kind']>(['via', 'waypoint', 'detour', 'pass']);

/** How the map shows a pin outside the week: a night mark as a dot, a shaping point not at all. */
export function pinLook(week: Week | null, point: MapPoint): 'shown' | 'dot' | 'hidden' {
    if (!week) return 'shown';
    if (point.kind === 'night' || point.kind === 'dayend') return week.nights.includes(point.night!) ? 'shown' : 'dot';
    return shaping.has(point.kind) && point.day !== undefined && !inside(week, point.day) ? 'hidden' : 'shown';
}
