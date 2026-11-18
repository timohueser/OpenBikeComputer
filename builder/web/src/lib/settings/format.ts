import { settings } from './settings.svelte';

// Each function reads `settings.units`, so markup that calls it follows a change.

const MILE = 1609.344;
const FOOT = 0.3048;
/** A number and its unit stay on one line. */
const SPACE = '\u00a0';
/** A value that is not a number shows as a dash. */
const NONE = '–';
// One format for each count of digits: a list of routes formats hundreds of numbers, and a new format for each is slow.
const formats = [0, 1].map(digits => new Intl.NumberFormat('en', { minimumFractionDigits: digits, maximumFractionDigits: digits }));
// Rounds before it formats, so that "-0" does not show and the digits follow the rounded value.
const number = (value: number, digits: 0 | 1) => formats[digits].format(Math.round(value * 10 ** digits) / 10 ** digits + 0);
/** One decimal under 100, whole numbers from 100. */
const tenths = (value: number) => Math.abs(Math.round(value * 10)) >= 1000 ? number(value, 0) : number(value, 1);

/** A distance in the units of the rider: kilometres or miles. */
export function distanceIn(metres: number): number {
    return settings.units === 'mi' ? metres / MILE : metres / 1000;
}

/** A height in the units of the rider: `heightUnit()`. */
export function heightIn(metres: number): number {
    return settings.units === 'mi' ? metres / FOOT : metres;
}

export function heightUnit(): 'm' | 'ft' {
    return settings.units === 'mi' ? 'ft' : 'm';
}

/** "km 41.6", "mi 25.8": a place along a route, by its distance from the start. */
export function formatAlong(metres: number): string {
    return Number.isFinite(metres) ? `${settings.units}${SPACE}${tenths(distanceIn(metres))}` : NONE;
}

/** "400 m", "12.3 km", "123 km", "7.6 mi". With kilometres, a distance under 1 km shows in metres; with miles, a distance above 0 shows at least "0.1 mi". */
export function formatDistance(metres: number): string {
    if (!Number.isFinite(metres)) return NONE;
    if (settings.units === 'km') return Math.abs(Math.round(metres)) < 1000 ? `${number(metres, 0)}${SPACE}m` : `${tenths(metres / 1000)}${SPACE}km`;
    const miles = distanceIn(metres);
    return `${metres > 0 && miles < 0.05 ? '0.1' : tenths(miles)}${SPACE}mi`;
}

/** "1,240 m", "4,068 ft": a climb, a descent or a highest point. */
export function formatHeight(metres: number): string {
    return Number.isFinite(metres) ? `${number(heightIn(metres), 0)}${SPACE}${heightUnit()}` : NONE;
}

/** "1,240", "7.5": a number with no unit, such as a count or the value over a bar. One decimal for a part under 10. */
export function formatCount(value: number): string {
    return Number.isFinite(value) ? number(value, Number.isInteger(value) || Math.abs(value) >= 10 ? 0 : 1) : NONE;
}

/** "5h 20m", from whole minutes. */
export const formatMinutes = (minutes: number) => `${Math.floor(minutes / 60)}h ${minutes % 60}m`;

/** "5h 20m", "1,105h": the hours of a total, in whole hours from 100 hours. */
export function formatHours(seconds: number): string {
    if (!Number.isFinite(seconds)) return NONE;
    const minutes = Math.round(seconds / 60);
    return minutes >= 6000 ? `${number(minutes / 60, 0)}h` : formatMinutes(minutes);
}

/** "7 %", "−4 %": a grade in whole percent. */
export function formatPercent(percent: number): string {
    if (!Number.isFinite(percent)) return NONE;
    const rounded = Math.round(percent) + 0;
    return `${rounded < 0 ? '−' : ''}${Math.abs(rounded)} %`;
}

/** "19.0 km/h", "11.8 mph". */
export function formatSpeed(metresPerSecond: number): string {
    if (!Number.isFinite(metresPerSecond)) return NONE;
    return settings.units === 'mi' ? `${number(metresPerSecond * 3600 / MILE, 1)}${SPACE}mph` : `${number(metresPerSecond * 3.6, 1)}${SPACE}km/h`;
}

/** "09:05": the time of day of a Unix time in seconds, in the zone of the browser. */
export const formatClock = (seconds: number): string => new Date(seconds * 1000).toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit' });
