// The slope angle of the ground in classes from 30°, which the browser computes from the terrain tiles of the map.
import type { Map } from 'maplibre-gl';
import { config } from '../map-data';
import { DEM_MAX_ZOOM, DEM_TILE } from '../map-style';
import { decodedTiles, fetchTile } from './archive';
import { demTile } from './climate-terrain';
import { abgr } from './colour';
import type { Legend } from './data-layer';
import { latitudeAt } from './mercator';
import { Raster, SLOPE_ANGLE } from './raster';
import { imagePixels } from './terrain';

const TILE = 256;
/** The view zoom from which the layer draws. A map pixel is a terrain pixel there, and the map enlarges the tiles at deeper zooms. */
export const SLOPE_ANGLE_ZOOM = DEM_MAX_ZOOM;
/** The zoom of the 256 px map tiles that MapLibre loads from that view zoom: each is a quarter of a terrain tile. */
const TILE_ZOOM = DEM_MAX_ZOOM + 1;
const EQUATOR_M = 2 * Math.PI * 6_378_137;

/** Each class starts at its angle in degrees and ends where the next starts; ground below the first is clear. */
const CLASSES = [
    { from: 30, color: '#f2e500', label: '30–35°' },
    { from: 35, color: '#f59600', label: '35–40°' },
    { from: 40, color: '#e0001a', label: '40–45°' },
    { from: 45, color: '#8228a0', label: '> 45°' },
];
/** The square of the rise over the run where each class starts. */
const LIMITS = CLASSES.map(({ from }) => Math.tan(from * Math.PI / 180) ** 2);
/** ImageData words by class value; 0 is clear. */
const PALETTE = Uint32Array.from([0, ...CLASSES.map(({ color }) => abgr(color))]);

export const slopeLegend: Legend = { swatches: CLASSES.map(({ color, label }) => ({ color, label })) };

/**
 * Metres with the fraction, R × 256 + G + B / 256 − 32768, and NaN where the height is not known. Whole metres
 * change a slope near 30° by up to 2°.
 */
export function exactHeights(rgba: ArrayLike<number>, count: number): Float32Array {
    const out = new Float32Array(count);
    for (let i = 0; i < count; i++) out[i] = rgba[4 * i + 3] === 255 ? rgba[4 * i] * 256 + rgba[4 * i + 1] + rgba[4 * i + 2] / 256 - 32768 : NaN;
    return out;
}

/** The terrain tiles beside a tile; a pixel at the edge reads its neighbour from them. */
export interface Beside { north?: Float32Array | null; south?: Float32Array | null; west?: Float32Array | null; east?: Float32Array | null }

/** The rise over one pixel between the two neighbours of a pixel on one axis. The pixel stands in for a neighbour with no known height. */
function rise(before: number, at: number, after: number): number {
    const first = before === before, last = after === after;
    return ((last ? after : at) - (first ? before : at)) / (first && last ? 2 : 1);
}

/**
 * The class of each pixel of a terrain tile of `size` px: 0 below 30° and where the height is not known, else the
 * place of the class in the legend from 1. The slope of a pixel comes from its four neighbours. `pixelMetres` gives the
 * size of a pixel on the ground in a row of the tile.
 */
export function slopeClasses(heights: Float32Array, beside: Beside, pixelMetres: (row: number) => number, size = DEM_TILE): Uint8Array {
    // The tile with a ring of one pixel from the tiles beside it.
    const width = size + 2, ring = new Float32Array(width * width).fill(NaN);
    for (let y = 0; y < size; y++) {
        const row = (y + 1) * width;
        ring.set(heights.subarray(y * size, (y + 1) * size), row + 1);
        if (beside.west) ring[row] = beside.west[(y + 1) * size - 1];
        if (beside.east) ring[row + size + 1] = beside.east[y * size];
    }
    if (beside.north) ring.set(beside.north.subarray(size * (size - 1)), 1);
    if (beside.south) ring.set(beside.south.subarray(0, size), (size + 1) * width + 1);
    const classes = new Uint8Array(size * size);
    for (let y = 0; y < size; y++) {
        const run = pixelMetres(y) ** 2;
        for (let x = 0; x < size; x++) {
            const i = (y + 1) * width + x + 1, height = ring[i];
            if (height !== height) continue;
            const slope = (rise(ring[i - 1], height, ring[i + 1]) ** 2 + rise(ring[i - width], height, ring[i + width]) ** 2) / run;
            let value = 0;
            while (value < LIMITS.length && slope >= LIMITS[value]) value++;
            classes[y * size + x] = value;
        }
    }
    return classes;
}

/** Paints a 256 px map tile from the part of the classes of its terrain tile that starts at `left`, `top`, one pixel for one pixel. */
export function paintSlope(words: Uint32Array, classes: Uint8Array, { left, top }: { left: number; top: number }) {
    for (let py = 0; py < TILE; py++) {
        const row = (top + py) * DEM_TILE + left;
        for (let px = 0; px < TILE; px++) words[py * TILE + px] = PALETTE[classes[row + px]];
    }
}

/** The map's terrain tiles; the relief has loaded most of them, so the browser cache serves them. A tile takes 1 MiB. */
const terrain = decodedTiles((z, x, y, signal) => fetchTile(config.terrain, z, x, y, signal), async body => {
    const { data, width, height } = await imagePixels(body);
    return exactHeights(data, width * height);
}, 32 * 2 ** 20, tile => tile.byteLength);

async function draw(z: number, x: number, y: number, signal: AbortSignal) {
    const part = demTile(z, x, y);
    const heights = await terrain.tile(part.z, part.x, part.y, signal);
    if (!heights) return undefined;
    // A tile beside it that fails is as a missing one: the pixels at that edge read their own tile only.
    const [north, south, west, east] = await Promise.all([[0, -1], [0, 1], [-1, 0], [1, 0]].map(([dx, dy]) =>
        terrain.tile(part.z, part.x + dx, part.y + dy, signal).catch(error => { if (signal.aborted) throw error; return null; })));
    const world = DEM_TILE * 2 ** part.z;
    const pixelMetres = (row: number) => EQUATOR_M / world * Math.cos(latitudeAt(part.y * DEM_TILE + row + 0.5, world) * Math.PI / 180);
    const image = new ImageData(TILE, TILE);
    paintSlope(new Uint32Array(image.data.buffer), slopeClasses(heights, { north, south, west, east }, pixelMetres), part);
    return image;
}

/** The slope angle layer of one map. It has one look: the colours are the same in each theme. */
export class SlopeAngle {
    private raster = new Raster({
        id: SLOPE_ANGLE,
        extent: async () => ({ minzoom: TILE_ZOOM, maxzoom: TILE_ZOOM, bounds: config.bounds, attribution: config.terrain_attribution }),
        draw: (_look, z, x, y, signal) => draw(z, x, y, signal),
        minzoom: SLOPE_ANGLE_ZOOM,
        paint: { 'raster-opacity': 0.55, 'raster-resampling': 'nearest' },
        // The layer is decoration, as the relief: a terrain tile that fails leaves its tile clear.
        report: error => { if (error) console.warn('Planner slope angle:', error); },
    });

    sync(map: Map, shown: boolean) {
        this.raster.sync(map, shown, 'slope');
    }
}
