import type { Coordinate } from '../map-types';

/** The Web Mercator position of a coordinate in a world `size` pixels wide: x east from 180° W, y south from the north edge. */
export function worldPixel([longitude, latitude]: Coordinate, size: number): [number, number] {
    return [(longitude + 180) / 360 * size, (0.5 - Math.asinh(Math.tan(latitude * Math.PI / 180)) / (2 * Math.PI)) * size];
}

/** The latitude of row `y` in a world `size` pixels wide. */
export function latitudeAt(y: number, size: number): number {
    return Math.atan(Math.sinh(Math.PI * (1 - 2 * y / size))) * 180 / Math.PI;
}
