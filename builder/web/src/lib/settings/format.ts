import { settings } from './settings.svelte';

// Each function reads `settings.units`, so markup that calls it follows a change.

const MILE = 1609.344;
const FOOT = 0.3048;
/** A value that is not a number shows as a dash. */
const NONE = '–';
// Rounds before it formats, so that "-0" does not show and the digits follow the rounded value.
const number = (value: number, digits: number) =>
    (Math.round(value * 10 ** digits) / 10 ** digits + 0).toLocaleString('en', { minimumFractionDigits: digits, maximumFractionDigits: digits });
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

/** "400 m", "12.3 km", "123 km", "7.6 mi". With kilometres, a distance under 1 km shows in metres; with miles, a distance above 0 shows at least "0.1 mi". */
export function formatDistance(metres: number): string {
    if (!Number.isFinite(metres)) return NONE;
    if (settings.units === 'km') return Math.abs(Math.round(metres)) < 1000 ? `${number(metres, 0)} m` : `${tenths(metres / 1000)} km`;
    const miles = distanceIn(metres);
    return `${metres > 0 && miles < 0.05 ? '0.1' : tenths(miles)} mi`;
}

/** "1,240 m", "4,068 ft": a climb, a descent or a highest point. */
export function formatHeight(metres: number): string {
    return Number.isFinite(metres) ? `${number(heightIn(metres), 0)} ${heightUnit()}` : NONE;
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
    return settings.units === 'mi' ? `${number(metresPerSecond * 3600 / MILE, 1)} mph` : `${number(metresPerSecond * 3.6, 1)} km/h`;
}
