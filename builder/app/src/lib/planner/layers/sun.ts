import type { Coordinate } from '../map-types';
import { latitudeAt, worldPixel } from './mercator';

export const DEM_ZOOM = 12, INDEX_ZOOM = 10, TILE = 512;
export const SUN = 0, SHADE = 1, NIGHT = 2, UNKNOWN = 3;
export const OUTSIDE = 4, TERRAIN_MAP_ZOOM = 7;
const RAD = Math.PI / 180, RADIUS = 6371008.8, CURVE = 1 / (2 * RADIUS);

export interface SunPosition { bearing: number; altitude: number }
export interface SunDay { states: Uint8Array; daylight: [number, number] | null; timezone: string }
export interface SunStats { cpuMs: number; requests: number; bytes: number; decodedBytes: number; rays: number; nodes: number; leaves: number; cacheHits: number }
export interface SunMeta { sun_format: number; dem_zoom: number; index_zoom: number; distance_m: number; timezone: string; bounds: [number, number, number, number]; attribution: string; horizon_zoom: number; horizon_samples: number; horizon_directions: number; horizon_step: number }

/** Wide views show global day/night without terrain. Terrain shade stops at the region edge. */
export function mapVisibility([lon, lat]: Coordinate, sun: SunPosition, zoom: number, bounds: SunMeta['bounds'], terrain: () => number): number {
    if (zoom < TERRAIN_MAP_ZOOM) return sun.altitude <= 0 ? NIGHT : SUN;
    if (lon < bounds[0] || lat < bounds[1] || lon > bounds[2] || lat > bounds[3]) return OUTSIDE;
    if (sun.altitude <= 0) return NIGHT;
    return terrain();
}

/** Linear interpolation across neighbouring bearings and grid centres. Unknown bounds stay unknown. */
export function horizonVisibility(read: (x: number, y: number, direction: number) => number, x: number, y: number, sun: SunPosition, directions: number, step: number): number {
    if (sun.altitude <= 0) return NIGHT;
    const x0 = Math.floor(x), y0 = Math.floor(y), fx = x - x0, fy = y - y0;
    const bearing = ((sun.bearing / (2 * Math.PI) % 1) + 1) % 1 * directions, first = Math.floor(bearing), mix = bearing - first;
    let horizon = 0;
    for (let dy = 0; dy < 2; dy++) for (let dx = 0; dx < 2; dx++) {
        const weight = (dx ? fx : 1 - fx) * (dy ? fy : 1 - fy);
        if (weight === 0) continue;
        const a = read(x0 + dx, y0 + dy, first), b = read(x0 + dx, y0 + dy, (first + 1) % directions);
        if (a === 255 && mix < 1 || b === 255 && mix > 0) return UNKNOWN;
        horizon += weight * (a * (1 - mix) + b * mix) * step;
    }
    return sun.altitude / RAD > horizon ? SUN : SHADE;
}

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

export function solarPosition([lon, lat]: Coordinate, terms: ReturnType<typeof solarTerms>): SunPosition {
    const hour = ((terms.minutes + terms.equation + 4 * lon) / 4 - 180) * RAD;
    const phi = lat * RAD, dec = terms.declination;
    const altitude = Math.asin(Math.max(-1, Math.min(1, Math.sin(phi) * Math.sin(dec) + Math.cos(phi) * Math.cos(dec) * Math.cos(hour))));
    return { altitude, bearing: Math.atan2(Math.sin(hour), Math.cos(hour) * Math.sin(phi) - Math.tan(dec) * Math.cos(phi)) + Math.PI };
}

/** Resolve a wall clock in the region's IANA zone, independent of the browser's zone. DST gaps are rejected. */
export function instantAt(date: string, minute: number, timezone: string): number {
    const target = Date.parse(`${date}T00:00:00Z`) + minute * 60000;
    const formatter = new Intl.DateTimeFormat('en-CA', { timeZone: timezone, year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', hourCycle: 'h23' });
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

/** Coordinates of the native pixel centres; bounds use this grid, not lower-zoom height samples. */
export function nativePoint(coordinate: Coordinate): [number, number] {
    const [x, y] = worldPixel(coordinate, TILE * 2 ** DEM_ZOOM);
    return [x - .5, y - .5];
}
export function coordinateAt(x: number, y: number, z: number, size: number): Coordinate {
    const n = size * 2 ** z;
    return [x / n * 360 - 180, latitudeAt(y, n)];
}

export interface Surface {
    /** `level=0` is a native DEM vertex. Levels 2..12 are conservative block maxima. */
    height(level: number, x: number, y: number): number;
}

/** Exact maximum of a bilinear terrain patch minus a curved solar ray over a segment. */
export function patchBlocks(h: number[], fx: number, fy: number, dx: number, dy: number, slope: number, observer: number, lo: number, hi: number): boolean {
    const e = h[1] - h[0], n = h[2] - h[0], cross = h[3] - h[2] - h[1] + h[0];
    const a = cross * dx * dy - CURVE;
    const b = e * dx + n * dy + cross * (fx * dy + fy * dx) - slope;
    const c = h[0] + e * fx + n * fy + cross * fx * fy - observer;
    const value = (t: number) => (a * t + b) * t + c;
    let maximum = Math.max(value(lo), value(hi));
    const peak = -b / (2 * a);
    if (a < 0 && peak > lo && peak < hi) maximum = Math.max(maximum, value(peak));
    return maximum > 0;
}

/** Hierarchical traversal. Missing data is unknown; a confirmed blocker is shade even beyond a gap. */
export function visibility(surface: Surface, coordinate: Coordinate, sun: SunPosition, distance: number, stats?: SunStats, skip = true): number {
    if (sun.altitude <= 0) return NIGHT;
    const [px, py] = nativePoint(coordinate), x0 = Math.floor(px), y0 = Math.floor(py);
    const h = [surface.height(0, x0, y0), surface.height(0, x0 + 1, y0), surface.height(0, x0, y0 + 1), surface.height(0, x0 + 1, y0 + 1)];
    if (h.some(value => !Number.isFinite(value))) return UNKNOWN;
    const fx = px - x0, fy = py - y0;
    const observer = h[0] * (1 - fx) * (1 - fy) + h[1] * fx * (1 - fy) + h[2] * (1 - fx) * fy + h[3] * fx * fy + 1;
    const pitch = 2 * Math.PI * 6378137 * Math.cos(coordinate[1] * RAD) / (TILE * 2 ** DEM_ZOOM);
    const dx = Math.sin(sun.bearing) / pitch, dy = -Math.cos(sun.bearing) / pitch, slope = Math.tan(sun.altitude);
    let lo = 0, unknown = false;
    if (stats) stats.rays++;
    while (lo < distance) {
        let level = skip ? 12 : 0;
        while (true) {
            const size = 2 ** level, x = Math.floor((px + dx * (lo + 1e-5)) / size), y = Math.floor((py + dy * (lo + 1e-5)) / size);
            const tx = dx > 0 ? ((x + 1) * size - px) / dx : dx < 0 ? (x * size - px) / dx : Infinity;
            const ty = dy > 0 ? ((y + 1) * size - py) / dy : dy < 0 ? (y * size - py) / dy : Infinity;
            const hi = Math.min(tx, ty, distance);
            if (stats) stats.nodes++;
            if (level >= 2) {
                if (surface.height(level, x, y) < observer + slope * lo + CURVE * lo * lo) { lo = hi; break; }
                level = level === 2 ? 0 : level - 1;
                continue;
            }
            if (stats) stats.leaves++;
            const vertices = [surface.height(0, x, y), surface.height(0, x + 1, y), surface.height(0, x, y + 1), surface.height(0, x + 1, y + 1)];
            if (vertices.some(value => !Number.isFinite(value))) unknown = true;
            else if (patchBlocks(vertices, px - x, py - y, dx, dy, slope, observer, lo, hi)) return SHADE;
            lo = hi;
            break;
        }
    }
    return unknown ? UNKNOWN : SUN;
}
