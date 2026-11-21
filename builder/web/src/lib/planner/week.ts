import type { ItineraryDay } from './editor';
import type { MapPoint } from './map-types';

/** The week in focus of a long trip: 7 calendar days, rest days included. The day list, the elevation profile and the
 * night search show only these days. */
export interface Week {
    /** The first and the last calendar day. */
    start: number;
    end: number;
    /** The riding numbers of the riding days inside. */
    ridingDays: number[];
    /** The nights to search: the riding days inside and the night before the week. */
    nights: number[];
}

const length = 7;
/** A trip with fewer riding days shows all its days, with no week. */
export const weekFrom = 15;

/** The first day of a week in a trip of `days` calendar days. */
const clamp = (days: number, start: number) => Math.max(1, Math.min(days - length + 1, start));

/** The week of `itinerary` that starts at calendar day `start`, held inside the trip; null for a trip of fewer than
 * `weekFrom` riding days. */
export function weekOf(itinerary: readonly ItineraryDay[], start: number): Week | null {
    if (itinerary.filter(day => !day.rest).length < weekFrom) return null;
    const first = clamp(itinerary.length, start), inside = itinerary.slice(first - 1, first - 1 + length);
    const ridingDays = inside.flatMap(day => day.rest ? [] : [day.ridingNumber]);
    // The night before a rest day is the night of the riding day it follows.
    const before = itinerary[first - 2]?.ridingNumber;
    return { start: first, end: first + length - 1, ridingDays, nights: [...before ? [before] : [], ...ridingDays] };
}

/** The moves of the week in a trip of `days` calendar days; each gives the new first day. */
export const centredOn = (days: number, day: number) => clamp(days, day - 3);
export const step = (days: number, start: number, by: number) => clamp(days, start + by);
/** The least move that puts calendar day `day` inside the week. */
export const containing = (days: number, start: number, day: number) => clamp(days, Math.min(day, Math.max(start, day - length + 1)));
export const home = 1;
export const end = (days: number) => clamp(days, days);

/** How the map draws a riding day: in its colour inside the week, quiet outside it, and lit under the pointer on the strip. */
export function dayLook(week: Week | null, lit: number | null, day: number): 'colour' | 'quiet' | 'lit' {
    return day === lit ? 'lit' : !week || week.ridingDays.includes(day) ? 'colour' : 'quiet';
}

const shaping = new Set<MapPoint['kind']>(['via', 'waypoint', 'detour', 'pass']);

/** How the map shows a pin outside the week: a night mark as a dot, a shaping point not at all. */
export function pinLook(week: Week | null, point: MapPoint): 'shown' | 'dot' | 'hidden' {
    if (!week) return 'shown';
    if (point.kind === 'night' || point.kind === 'dayend') return week.nights.includes(point.night!) ? 'shown' : 'dot';
    return shaping.has(point.kind) && point.day !== undefined && !week.ridingDays.includes(point.day) ? 'hidden' : 'shown';
}
