// Terrain heights for the temperature map, read from the same Terrarium tiles as the map's terrain source.
import { DEM_MAX_ZOOM, DEM_TILE } from '../map-style';

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

/**
 * Heights in metres of the 256 × 256 nearest pixels of the part of a 512 px Terrarium RGBA tile that
 * a map tile covers: R × 256 + G + B ÷ 256 − 32768.
 */
export function cropHeights(rgba: ArrayLike<number>, { scale, left, top }: { scale: number; left: number; top: number }): Float32Array {
    const step = DEM_TILE / scale / TILE;
    const out = new Float32Array(TILE * TILE);
    for (let py = 0; py < TILE; py++) {
        const row = (top + Math.floor(py * step)) * DEM_TILE + left;
        for (let px = 0; px < TILE; px++) {
            const p = 4 * (row + Math.floor(px * step));
            out[py * TILE + px] = rgba[p] * 256 + rgba[p + 1] + rgba[p + 2] / 256 - 32768;
        }
    }
    return out;
}

/**
 * Heights in metres of the 256 × 256 pixels of map tile z/x/y, row by row from the north-west pixel;
 * undefined where the terrain service has no tile. `template` is the map's Terrarium URL, so the
 * browser cache serves tiles that the map has loaded.
 */
export async function tileHeights(template: string, z: number, x: number, y: number, signal?: AbortSignal): Promise<Float32Array | undefined> {
    const dem = demTile(z, x, y);
    const response = await fetch(template.replace('{z}', String(dem.z)).replace('{x}', String(dem.x)).replace('{y}', String(dem.y)), { signal });
    if (!response.ok || response.status === 204) return undefined;
    // Without these options the browser may convert colours or premultiply, which changes the heights.
    const image = await createImageBitmap(await response.blob(), { colorSpaceConversion: 'none', premultiplyAlpha: 'none' });
    const context = new OffscreenCanvas(DEM_TILE, DEM_TILE).getContext('2d', { willReadFrequently: true })!;
    context.drawImage(image, 0, 0, DEM_TILE, DEM_TILE);
    image.close();
    return cropHeights(context.getImageData(0, 0, DEM_TILE, DEM_TILE).data, dem);
}
