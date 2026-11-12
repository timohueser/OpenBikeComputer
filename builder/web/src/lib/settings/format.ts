import type { Bike } from '../library/records';
import { PACES, settings } from './settings.svelte';

// Each function reads `settings.units` or `settings.pace`, so markup that calls it follows a change.

const MILE = 1609.344;
const FOOT = 0.3048;
const number = (value: number, digits: number) =>
    value.toLocaleString('en', { minimumFractionDigits: digits, maximumFractionDigits: digits });

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

/** "12.3 km", "123 km", "7.6 mi". With kilometres, a distance under 1 km shows in metres. */
export function formatDistance(metres: number): string {
    if (settings.units === 'km' && metres < 1000) return `${Math.round(metres)} m`;
    const value = distanceIn(metres);
    return `${number(value, value < 100 ? 1 : 0)} ${settings.units}`;
}

/** "1,240 m", "4,068 ft": a climb, a descent or a highest point. */
export function formatHeight(metres: number): string {
    return `${number(heightIn(metres), 0)} ${heightUnit()}`;
}

/** "19.0 km/h", "11.8 mph". */
export function formatSpeed(metresPerSecond: number): string {
    return settings.units === 'mi' ? `${number(metresPerSecond * 3600 / MILE, 1)} mph` : `${number(metresPerSecond * 3.6, 1)} km/h`;
}

/** A routing time at the pace of the rider. The pace is a cycling speed, so a time on foot does not change. */
export function plannedSeconds(seconds: number, bike: Bike): number {
    return bike === 'hiking' ? seconds : seconds * PACES.moderate.kmh / PACES[settings.pace].kmh;
}
