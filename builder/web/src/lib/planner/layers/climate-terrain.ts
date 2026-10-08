// Terrain heights for the temperature map, read from the same Terrarium tiles as the map's terrain source.
import { DEM_MAX_ZOOM, DEM_TILE } from '../map-style';
import type { Coordinate } from '../map-types';
import { worldPixel } from './mercator';
import { UNKNOWN_HEIGHT } from './terrain';

const TILE = 256;

/**
 * The DEM tile for a 256 px map tile and the part of it that the map tile covers. A 512 px source
 * loads zoom z − 1 for a view at zoom z, so this is the tile the map's terrain already requested,
 * at one DEM pixel per map pixel up to the DEM zoom.
 */
export function demTile(z: number, x: number, y: number): { z: number; x: number; y: number; scale: number; left: number; top: number } {
    const dz = Math.min(Math.max(z - 1, 0), DEM_MAX_ZOOM), scale = 2 ** (z - dz);
    const part = DEM_TILE / scale;
    return { z: dz, x: Math.floor(x / scale), y: Math.floor(y / scale), scale, left: (x % scale) * part, top: (y % scale) * part };
}

/** The DEM zoom that the 512 px relief loads at a map zoom: MapLibre rounds the zoom for raster sources. */
export function reliefZoom(zoom: number): number {
    return Math.min(Math.max(Math.round(zoom), 0), DEM_MAX_ZOOM);
}

/** The DEM tile at zoom z that holds a coordinate, and the index of its pixel there. */
export function demPixel(coordinate: Coordinate, z: number): { x: number; y: number; pixel: number } {
    const [px, py] = worldPixel(coordinate, DEM_TILE * 2 ** z).map(Math.floor);
    return { x: Math.floor(px / DEM_TILE), y: Math.floor(py / DEM_TILE), pixel: (py % DEM_TILE) * DEM_TILE + (px % DEM_TILE) };
}

/** Metres; NaN where the height is unknown. */
export const metres = (height: number) => height === UNKNOWN_HEIGHT ? NaN : height;

/** Heights of the 256 × 256 nearest pixels of the part of a 512 px DEM tile that a map tile covers. */
export function cropHeights(heights: Int16Array, { scale, left, top }: { scale: number; left: number; top: number }): Float32Array {
    const step = DEM_TILE / scale / TILE;
    const out = new Float32Array(TILE * TILE);
    for (let py = 0; py < TILE; py++) {
        const row = (top + Math.floor(py * step)) * DEM_TILE;
        for (let px = 0; px < TILE; px++) out[py * TILE + px] = metres(heights[row + left + Math.floor(px * step)]);
    }
    return out;
}
