import type { Coordinate } from './map-types';

const RAD = Math.PI / 180;

/** Geometric solar centre, Meeus equations used by NOAA's solar calculator. Angles are radians. */
export function solarTerms(instant: number): { declination: number; equation: number; minutes: number } {
    const t = (instant / 86400000 + 2440587.5 - 2451545) / 36525;
    const l = (280.46646 + t * (36000.76983 + t * .0003032)) % 360;
    const m = 357.52911 + t * (35999.05029 - .0001537 * t);
    const e = .016708634 - t * (.000042037 + .0000001267 * t);
    const c = Math.sin(m * RAD) * (1.914602 - t * (.004817 + .000014 * t))
        + Math.sin(2 * m * RAD) * (.019993 - .000101 * t) + Math.sin(3 * m * RAD) * .000289;
    const omega = (125.04 - 1934.136 * t) * RAD;
    const lambda = (l + c - .00569 - .00478 * Math.sin(omega)) * RAD;
    const obliquity = (23 + (26 + (21.448 - t * (46.815 + t * (.00059 - t * .001813))) / 60) / 60 + .00256 * Math.cos(omega)) * RAD;
    const y = Math.tan(obliquity / 2) ** 2;
    const equation = 4 / RAD * (y * Math.sin(2 * l * RAD) - 2 * e * Math.sin(m * RAD)
        + 4 * e * y * Math.sin(m * RAD) * Math.cos(2 * l * RAD) - .5 * y * y * Math.sin(4 * l * RAD) - 1.25 * e * e * Math.sin(2 * m * RAD));
    const minutes = ((instant % 86400000) + 86400000) % 86400000 / 60000;
    return { declination: Math.asin(Math.sin(obliquity) * Math.sin(lambda)), equation, minutes };
}

/** The sun's altitude, and its bearing clockwise from north. */
export function solarPosition([lon, lat]: Coordinate, terms: ReturnType<typeof solarTerms>): { bearing: number; altitude: number } {
    const hour = ((terms.minutes + terms.equation + 4 * lon) / 4 - 180) * RAD;
    const phi = lat * RAD, dec = terms.declination;
    const altitude = Math.asin(Math.max(-1, Math.min(1, Math.sin(phi) * Math.sin(dec) + Math.cos(phi) * Math.cos(dec) * Math.cos(hour))));
    return { altitude, bearing: Math.atan2(Math.sin(hour), Math.cos(hour) * Math.sin(phi) - Math.tan(dec) * Math.cos(phi)) + Math.PI };
}

// Making a formatter is slow, and daylight() resolves 288 instants.
const formatters = new Map<string, Intl.DateTimeFormat>();

/** Resolve a wall clock in the region's IANA zone, independent of the browser's zone. DST gaps are rejected. */
export function instantAt(date: string, minute: number, timezone: string): number {
    const target = Date.parse(`${date}T00:00:00Z`) + minute * 60000;
    let formatter = formatters.get(timezone);
    if (!formatter) formatters.set(timezone, formatter = new Intl.DateTimeFormat('en-CA', { timeZone: timezone, year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', hourCycle: 'h23' }));
    const local = (ms: number) => {
        const parts = Object.fromEntries(formatter.formatToParts(ms).map(p => [p.type, p.value]));
        return Date.parse(`${parts.year}-${parts.month}-${parts.day}T${parts.hour}:${parts.minute}:00Z`);
    };
    let value = target;
    for (let i = 0; i < 3; i++) value += target - local(value);
    if (local(value) !== target) throw new Error('This clock time does not exist when the clocks change. Choose another time.');
    return value;
}

export const clock = (minute: number) => `${String(Math.floor(minute / 60)).padStart(2, '0')}:${String(minute % 60).padStart(2, '0')}`;

/** The wall-clock span of the five-minute bins whose middle has the sun's upper limb above the horizon, in minutes; null in a polar night. */
export function daylight(coordinate: Coordinate, date: string, timezone: string): [number, number] | null {
    let first = -1, last = -1;
    for (let minute = 0; minute < 1440; minute += 5) {
        let instant: number;
        try { instant = instantAt(date, minute + 2, timezone); }
        catch { continue; }
        if (solarPosition(coordinate, solarTerms(instant)).altitude > -.833 * RAD) { if (first < 0) first = minute; last = minute + 5; }
    }
    return first < 0 ? null : [first, last];
}
