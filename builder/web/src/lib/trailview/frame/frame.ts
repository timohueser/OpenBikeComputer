import { latitudeAt } from '../../planner/layers/mercator';

export const EARTH_RADIUS = 6_371_000;
/** The refraction coefficient of the atmosphere: light bends down, so the Earth looks flatter. */
export const REFRACTION = 0.13;
/** Metres of drop per square metre of horizontal distance, for the vertex shader. */
export const CURVATURE = (1 - REFRACTION) / (2 * EARTH_RADIUS);

const RAD = Math.PI / 180;

/**
 * The local metric frame: an azimuthal equidistant projection about an origin, in metres east and
 * north. Its scale error is d² / 6R², so 0.04 % at 300 km. Heights are metres above sea level.
 */
export interface Frame {
    readonly lon: number;
    readonly lat: number;
    local(lon: number, lat: number): [number, number];
    lonLat(east: number, north: number): [number, number];
}

export function frameAt(lon0: number, lat0: number): Frame {
    const sin0 = Math.sin(lat0 * RAD), cos0 = Math.cos(lat0 * RAD);
    return {
        lon: lon0, lat: lat0,
        local(lon, lat) {
            const phi = lat * RAD, dl = (lon - lon0) * RAD;
            const x = Math.cos(phi) * Math.sin(dl);
            const y = cos0 * Math.sin(phi) - sin0 * Math.cos(phi) * Math.cos(dl);
            const sinc = Math.hypot(x, y);
            if (sinc === 0) return [0, 0];
            const k = EARTH_RADIUS * Math.atan2(sinc, sin0 * Math.sin(phi) + cos0 * Math.cos(phi) * Math.cos(dl)) / sinc;
            return [k * x, k * y];
        },
        lonLat(east, north) {
            const rho = Math.hypot(east, north);
            if (rho === 0) return [lon0, lat0];
            const c = rho / EARTH_RADIUS, sinc = Math.sin(c), cosc = Math.cos(c);
            const lat = Math.asin(cosc * sin0 + north * sinc * cos0 / rho);
            const lon = lon0 * RAD + Math.atan2(east * sinc, rho * cos0 * cosc - north * sin0 * sinc);
            return [lon / RAD, lat / RAD];
        },
    };
}

/** The longitude and latitude of the Web Mercator position (x, y) of tile zoom z, in tiles. */
export function tileLonLat(x: number, y: number, z: number): [number, number] {
    const n = 2 ** z;
    return [x / n * 360 - 180, latitudeAt(y, n)];
}
