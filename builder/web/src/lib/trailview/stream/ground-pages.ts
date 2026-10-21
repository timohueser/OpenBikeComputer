// The ground pages of a material tile on a date: a 512 px tile of zoom z gives the 256 × 256 texel
// pages of its four nodes one level finer, with every other layer resampled onto its texels. Workers
// use this module.

import { PAGE } from './heights';

const TILE = 2 * PAGE;
/** The material byte: bit 6 barren, bit 7 no data (specs/trailview-tiles.md#ground). */
const BARREN_BIT = 0x40, NO_DATA_BIT = 0x80, MATERIAL_BITS = 0x3f;
/** The region byte of no data. */
const NO_REGION = 255;
/** Snow bytes (specs/planner-snow-tiles.md). */
const WHOLE_SEASON = 254, NO_SNOW_DATA = 255;
/** The colour alpha codes: fewer than 3 looks, and a quarter that saw snow. */
const UNSEEN = 0, SNOW_MARK = 1;

/** Decoded pixels of tile z/x/y: `channels` bytes per pixel, `size` pixels a side. */
export interface Raster { z: number; x: number; y: number; size: number; channels: number; data: Uint8Array }

/** The date of the pages: the weight of the second quarter, the snow day index, and per region its snow_share. */
export interface GroundDate { weight: number; snowIndex: number; shares: ArrayLike<number> }

/** What the pages of one material tile come from; a layer without a tile there is undefined. */
export interface GroundInputs {
    z: number; x: number; y: number;
    material?: Raster;
    lithology?: Raster;
    region?: Raster;
    /** The two quarters around the date, and the highest NDVI of the year. */
    quarters?: [Raster | undefined, Raster | undefined];
    ndvimax?: Raster;
    /** The snow tiles over the material tile: one coarser tile, or four tiles one zoom finer. */
    snow?: (Raster | undefined)[];
    /** Seasons in a snow tile. */
    seasons: number;
    date: GroundDate;
    /** The region id that stands for no data. */
    world: number;
}

/**
 * One page: per texel `cells` (material, lithology, region, barren × 255), `colour` (the satellite on
 * the date, and its NDVI code: 0 where no quarter looked) and `snow` (snow by the region's snow_share,
 * snow data known, the satellite's snow mark, the share of seasons with snow × known).
 */
export interface GroundPage { cells: Uint8Array<ArrayBuffer>; colour: Uint8Array<ArrayBuffer>; snow: Uint8Array<ArrayBuffer> }

/** Where texel centre i of the material tile's grid falls in a raster, in its pixels, along one axis. */
function axis(raster: Raster, z: number, tile: number, axisOf: 'x' | 'y'): Float64Array {
    const scale = 2 ** (z - raster.z), out = new Float64Array(TILE);
    for (let i = 0; i < TILE; i++) out[i] = ((tile + (i + 0.5) / TILE) / scale - raster[axisOf]) * raster.size - 0.5;
    return out;
}

const clampIndex = (v: number, size: number) => Math.min(size - 1, Math.max(0, v));

/** The nearest pixel of a raster at each texel of the grid, as an index of the raster. */
function nearest(raster: Raster, z: number, x: number, y: number): Int32Array {
    const u = axis(raster, z, x, 'x'), v = axis(raster, z, y, 'y'), out = new Int32Array(TILE * TILE);
    for (let j = 0; j < TILE; j++) {
        const row = clampIndex(Math.round(v[j]), raster.size) * raster.size;
        for (let i = 0; i < TILE; i++) out[j * TILE + i] = row + clampIndex(Math.round(u[i]), raster.size);
    }
    return out;
}

/** Values per raster pixel, resampled bilinearly onto the grid; the raster's edge repeats. */
function bilinear(raster: Raster, values: Float32Array, z: number, x: number, y: number): Float32Array {
    const u = axis(raster, z, x, 'x'), v = axis(raster, z, y, 'y'), n = raster.size, out = new Float32Array(TILE * TILE);
    for (let j = 0; j < TILE; j++) {
        const j0 = Math.floor(v[j]), fy = v[j] - j0, r0 = clampIndex(j0, n) * n, r1 = clampIndex(j0 + 1, n) * n;
        for (let i = 0; i < TILE; i++) {
            const i0 = Math.floor(u[i]), fx = u[i] - i0, c0 = clampIndex(i0, n), c1 = clampIndex(i0 + 1, n);
            const top = values[r0 + c0] + (values[r0 + c1] - values[r0 + c0]) * fx;
            const bottom = values[r1 + c0] + (values[r1 + c1] - values[r1 + c0]) * fx;
            out[j * TILE + i] = top + (bottom - top) * fy;
        }
    }
    return out;
}

/** One channel of a raster as floats. */
const channel = (raster: Raster, c: number) => Float32Array.from({ length: raster.size * raster.size }, (_, i) => raster.data[i * raster.channels + c]);

/**
 * The satellite on the date at each pixel of the two quarters, as the prototype's `groundOn` blends
 * them: a quarter that saw nothing hands its weight to the other. A quarter that saw snow measured no
 * NDVI of the ground under it; the nearer quarter's snow mark goes with the colour.
 */
export function dateColour(a: Raster | undefined, b: Raster | undefined, weight: number): { rgb: Float32Array[]; ndvi: Float32Array; seen: Float32Array; mark: Float32Array } {
    const raster = a ?? b!, count = raster.size * raster.size;
    const rgb = [0, 1, 2].map(() => new Float32Array(count)), ndvi = new Float32Array(count), seen = new Float32Array(count), mark = new Float32Array(count);
    const at = (q: Raster | undefined, i: number, c: number) => (q ? q.data[4 * i + c] : 0);
    for (let i = 0; i < count; i++) {
        const codes = [at(a, i, 3), at(b, i, 3)];
        const marked = codes.map(code => +(code === SNOW_MARK));
        const looked = [a, b].map((q, k) => +(codes[k] !== UNSEEN && Math.max(at(q, i, 0), at(q, i, 1), at(q, i, 2)) > 0));
        const w = [looked[0] * (1 - weight + weight * (1 - looked[1])), looked[1] * (weight + (1 - weight) * (1 - looked[0]))];
        const sum = w[0] + w[1];
        for (let c = 0; c < 3; c++) rgb[c][i] = (w[0] * at(a, i, c) + w[1] * at(b, i, c)) / 255;
        ndvi[i] = sum > 0 ? w.reduce((s, wk, k) => s + wk * (marked[k] ? 0 : (codes[k] - 1) / 127 - 1), 0) : 0.45;
        seen[i] = +(sum > 0);
        mark[i] = weight < 0.5 ? marked[0] : marked[1];
    }
    return { rgb, ndvi, seen, mark };
}

/** Per snow pixel: snow by the share rule of its region, whether the data knows, and the share × known. */
export function snowState(snow: Raster, seasons: number, index: number, regionOf: (pixel: number) => number, shares: ArrayLike<number>): Float32Array[] {
    const area = snow.size * snow.size, snowy = new Float32Array(area), known = new Float32Array(area), share = new Float32Array(area);
    for (let i = 0; i < area; i++) {
        let count = 0, white = 0;
        for (let s = 0; s < seasons; s++) {
            const onset = snow.data[2 * s * area + i], melt = snow.data[(2 * s + 1) * area + i];
            if (onset === NO_SNOW_DATA) continue;
            count++;
            if (onset === WHOLE_SEASON || (onset <= index && index <= melt)) white++;
        }
        if (!count) continue;
        known[i] = 1;
        share[i] = white / count;
        snowy[i] = +(white / count >= shares[regionOf(i)]);
    }
    return [snowy, known, share];
}

/** Four snow tiles of zoom z + 1, north-west first, as one raster of zoom z. */
export function joinSnow(tiles: (Raster | undefined)[], seasons: number, z: number, x: number, y: number): Raster {
    const half = tiles.find(Boolean)?.size ?? PAGE, size = 2 * half, data = new Uint8Array(2 * seasons * size * size).fill(NO_SNOW_DATA);
    tiles.forEach((tile, q) => {
        if (!tile) return;
        for (let plane = 0; plane < 2 * seasons; plane++) for (let j = 0; j < half; j++) {
            const from = plane * half * half + j * half, to = plane * size * size + (j + (q >> 1) * half) * size + (q & 1) * half;
            data.set(tile.data.subarray(from, from + half), to);
        }
    });
    return { z, x, y, size, channels: 1, data };
}

/** The four pages of a material tile, north-west first; null where the material has no data. */
export function groundPages(input: GroundInputs): (GroundPage | null)[] {
    const { z, x, y, material, date } = input;
    if (!material) return [null, null, null, null];
    const n = TILE * TILE, cells = new Uint8Array(4 * n), colour = new Uint8Array(4 * n), snow = new Uint8Array(4 * n);
    const pick = (raster: Raster | undefined) => (raster ? nearest(raster, z, x, y) : undefined);
    const lithology = pick(input.lithology), region = pick(input.region);
    const regionAt = (t: number) => {
        // `shares` has one entry per region of the rules: a code outside them is no data.
        const r = region ? input.region!.data[region[t] * input.region!.channels] : NO_REGION;
        return r === NO_REGION || r >= input.date.shares.length ? input.world : r;
    };
    for (let t = 0; t < n; t++) {
        const byte = material.data[t * material.channels];
        cells[4 * t] = byte & NO_DATA_BIT ? 0 : byte & MATERIAL_BITS;
        cells[4 * t + 1] = lithology ? input.lithology!.data[lithology[t] * input.lithology!.channels] : 0;
        cells[4 * t + 2] = regionAt(t);
        cells[4 * t + 3] = byte & BARREN_BIT ? 255 : 0;
    }
    const [a, b] = input.quarters ?? [];
    if (a || b) {
        const raster = (a ?? b)!, day = dateColour(a, b, date.weight), resample = (values: Float32Array) => bilinear(raster, values, z, x, y);
        const [r, g, bl] = day.rgb.map(resample), ndvi = resample(day.ndvi), seen = resample(day.seen), mark = resample(day.mark);
        for (let t = 0; t < n; t++) {
            colour.set([r[t], g[t], bl[t]].map(c => Math.round(255 * c)), 4 * t);
            colour[4 * t + 3] = seen[t] < 0.5 ? UNSEEN : Math.min(255, Math.max(2, Math.round((ndvi[t] + 1) * 127 + 1)));
            snow[4 * t + 2] = Math.round(255 * mark[t]);
        }
    }
    if (input.ndvimax) {
        // Barren: the year's highest NDVI is low, softly, where a quarter measured one.
        const most = bilinear(input.ndvimax, channel(input.ndvimax, 0).map(code => (code > 0 ? 1 - smoothstep(0.15, 0.35, (code - 1) / 127 - 1) : 0)), z, x, y);
        for (let t = 0; t < n; t++) cells[4 * t + 3] = Math.round(255 * most[t]);
    }
    if (input.snow?.length) {
        const tile = input.snow.length === 4 ? joinSnow(input.snow, input.seasons, z, x, y) : input.snow[0]!;
        // The region of a snow pixel is that of the material texel nearest its centre.
        const step = 2 ** (z - tile.z);
        const at = (pixel: number, from: number, to: number) => clampIndex(Math.floor(((pixel + 0.5) / tile.size + from) * step * TILE) - to * TILE, TILE);
        const regionOf = (pixel: number) => regionAt(at(Math.floor(pixel / tile.size), tile.y, y) * TILE + at(pixel % tile.size, tile.x, x));
        const state = snowState(tile, input.seasons, date.snowIndex, regionOf, date.shares).map(values => bilinear(tile, values, z, x, y));
        for (let t = 0; t < n; t++) {
            snow[4 * t] = Math.round(255 * state[0][t]);
            snow[4 * t + 1] = Math.round(255 * state[1][t]);
            snow[4 * t + 3] = Math.round(255 * state[2][t]);
        }
    }
    return [0, 1, 2, 3].map(q => {
        const page = (source: Uint8Array) => {
            const out = new Uint8Array(4 * PAGE * PAGE);
            for (let j = 0; j < PAGE; j++) {
                const from = 4 * ((j + (q >> 1) * PAGE) * TILE + (q & 1) * PAGE);
                out.set(source.subarray(from, from + 4 * PAGE), 4 * j * PAGE);
            }
            return out;
        };
        let any = false;
        for (let j = 0; j < PAGE && !any; j++) for (let i = 0; i < PAGE; i++) {
            if (!(material.data[((j + (q >> 1) * PAGE) * TILE + i + (q & 1) * PAGE) * material.channels] & NO_DATA_BIT)) { any = true; break; }
        }
        return any ? { cells: page(cells), colour: page(colour), snow: page(snow) } : null;
    });
}

function smoothstep(edge0: number, edge1: number, x: number): number {
    const t = Math.min(Math.max((x - edge0) / (edge1 - edge0), 0), 1);
    return t * t * (3 - 2 * t);
}
