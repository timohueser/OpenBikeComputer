// Snow history decoded per specs/planner-snow-tiles.md: the runs of bands of `snow` and `snow-levels`
// for the map, and the onset and melt-out of each season of `snow-seasons` for the samples.
import type { Coordinate } from '../map-types';
import { noData } from './colour';
import { columnDate, dateLabel, DAY_MS, type Chart, type Grid, type Swatch, type Theme } from './data-layer';
import { worldPixel } from './mercator';

/** Per season, `size` onset bytes, then `size` melt-out bytes. A `snow-seasons` tile has 4,096 items; a line has one per sample. */
export interface Planar { data: Uint8Array; size: number; seasons: number }

const DAYS = 183;
const NO_SNOW = 253, WHOLE_SEASON = 254, NO_DATA = 255;
/**
 * A class is the number of levels that hold the band of a date, 0 to `EVERY`. By the share f of the
 * seasons with data that had snow: 0, below 1/4, 1/4 to below 1/2, 1/2 to below 3/4, 3/4 to below 1, 1.
 */
export const EVERY = 5, UNKNOWN = 6;
const SEPTEMBER = Date.UTC(2001, 8, 1);

export interface SnowMeta { firstSeason: number; seasons: number; resolution: number; attribution: string }

/** The metadata of `snow`, `snow-levels` and `snow-seasons`. */
export function snowMeta(json: Record<string, unknown>): SnowMeta {
    const meta = { firstSeason: Number(json.first_season), seasons: Number(json.seasons), resolution: Number(json.resolution_m), attribution: String(json.attribution ?? '') };
    if (Number(json.snow_format) !== 1 || !Number.isInteger(meta.firstSeason) || !(meta.seasons > 0)) throw new Error('The snow archive has no valid snow_format 1 metadata.');
    return meta;
}

/** The max zoom of a `snow` or `snow-levels` tile of MODIS: it holds the per-year tiles of zoom 8. */
const MODIS_ZOOM = 7;
/** The zoom of the `snow-seasons` tiles that hold the per-year tiles of MODIS pixel for pixel. */
const MODIS_SEASONS_ZOOM = 10;

/**
 * The `snow` or `snow-levels` tile that covers tile z/x/y at `maxZoom` or less: its own, else, above
 * the MODIS zoom, the nearest ancestor of zoom 7 or more that the archive has. `data` is null where none covers it.
 */
export async function covering<T>(tile: (z: number, x: number, y: number) => Promise<T | null>, maxZoom: number, z: number, x: number, y: number): Promise<{ data: T | null; zoom: number }> {
    for (let zoom = Math.min(z, maxZoom); ; zoom--) {
        const scale = 2 ** (z - zoom), data = await tile(zoom, Math.floor(x / scale), Math.floor(y / scale));
        if (data || zoom <= MODIS_ZOOM) return { data, zoom };
    }
}

/** The `snow-seasons` tile z/x/y, else, from zoom 11, its ancestor of zoom 10, which has the MODIS data. */
export async function seasonsTile<T>(tile: (z: number, x: number, y: number) => Promise<T | null>, z: number, x: number, y: number): Promise<{ data: T | null; zoom: number }> {
    const data = await tile(z, x, y);
    if (data || z <= MODIS_SEASONS_ZOOM) return { data, zoom: z };
    const scale = 2 ** (z - MODIS_SEASONS_ZOOM);
    return { data: await tile(MODIS_SEASONS_ZOOM, Math.floor(x / scale), Math.floor(y / scale)), zoom: MODIS_SEASONS_ZOOM };
}

/** The most `snow-seasons` tiles that the samples of a route read at their zoom. */
export const ROUTE_TILES = 25;

/** The finest zoom, at most `maxZoom`, at which the coordinates fall in at most `ROUTE_TILES` tiles; else `minZoom`. */
export function routeZoom(coordinates: readonly Coordinate[], minZoom: number, maxZoom: number): number {
    // A power of two scales exactly, so these give the tiles of `worldPixel(coordinate, 2 ** zoom)`.
    const world = coordinates.map(coordinate => worldPixel(coordinate, 1)), tiles = new Set<number>();
    for (let zoom = maxZoom; zoom > minZoom; zoom--) {
        const size = 2 ** zoom;
        tiles.clear();
        for (const [x, y] of world) {
            tiles.add(Math.floor(x * size) * size + Math.floor(y * size));
            if (tiles.size > ROUTE_TILES) break;
        }
        if (tiles.size <= ROUTE_TILES) return zoom;
    }
    return minZoom;
}

/** The season and day index of an ISO date. A leap day counts as 28 February, so each season has 183 indices. */
export function seasonDay(date: string): { season: number; index: number } {
    const [year, month, day] = date.split('-').map(Number);
    const days = (Date.UTC(month >= 9 ? 2001 : 2002, month - 1, month === 2 ? Math.min(day, 28) : day) - SEPTEMBER) / DAY_MS;
    return { season: month >= 9 ? year : year - 1, index: Math.min(DAYS - 1, Math.floor(days / 2)) };
}

/** The band of a day index: band b holds the day indices 4b to 4b + 3. */
export function snowBand(index: number): number {
    return Math.floor(index / 4);
}

/** The day index on which band `band` counts the snow of each season. */
export function bandDay(band: number): number {
    return Math.min(4 * band + 2, 182);
}

/** The band day of a date: each count of the snow on a date uses it, as the map does. */
const dateDay = (date: string) => bandDay(snowBand(seasonDay(date).index));

/** The charts have one column per day index. */
export const COLUMNS = DAYS;
// Calendar columns of 2001 in season terms: Jan–Aug belong to season 2000, Sep–Dec to season 2001.
const columns = Array.from({ length: COLUMNS }, (_, column) => seasonDay(columnDate(column, 2001, COLUMNS)));

/** The class of `snow` seasons with snow out of `known` seasons with data, as the levels of the map count them. */
function shareClass(snow: number, known: number): number {
    if (!known) return UNKNOWN;
    if (!snow) return 0;
    if (snow === known) return EVERY;
    return 4 * snow >= 3 * known ? 4 : 2 * snow >= known ? 3 : 4 * snow >= known ? 2 : 1;
}

/** Seasons with snow on day `index` at item `i`, and seasons with data there. */
function snowSeasons(p: Planar, i: number, index: number): [number, number] {
    let snow = 0, known = 0;
    for (let s = 0; s < p.seasons; s++) {
        const onset = p.data[2 * s * p.size + i], melt = p.data[(2 * s + 1) * p.size + i];
        if (onset === NO_DATA || melt === NO_DATA) continue;
        known++;
        if (onset === WHOLE_SEASON || (onset <= index && index <= melt)) snow++;
    }
    return [snow, known];
}

/** The class of item `i` on band `band`, from the seasons with snow on its band day. */
export function snowClass(p: Planar, i: number, band: number): number {
    return shareClass(...snowSeasons(p, i, bandDay(band)));
}

/**
 * The first and the last band of the run of one level, for each pixel of a 512 × 512 tile in row
 * order: 253 where the level holds no band, 255 where no season has data.
 */
export interface Runs { first: Uint8Array; last: Uint8Array }

export const RUN_TILE = 512;
/** The index of the half level, `snow`, in the levels of `levelClass`. */
const HALF = 2;

// 253 and 255 are above every band, so a run with them holds none.
const holds = (runs: Runs, i: number, band: number) => runs.first[i] <= band && band <= runs.last[i];

/**
 * The class of pixel `i` on `band`: the count of levels, from `any` up, that hold the band without a
 * gap. A higher level that holds it above a lower one that does not adds nothing, so the classes stay
 * in order where the runs do not nest. `levels` is in the order any, quarter, half, three quarters, every.
 */
export function levelClass(levels: readonly Runs[], i: number, band: number): number {
    if (levels[HALF].first[i] === NO_DATA) return UNKNOWN;
    let level = 0;
    while (level < levels.length && holds(levels[level], i, band)) level++;
    return level;
}

// For each screen column and row: the two data pixels around its centre and the bilinear fraction
// between them. Module-level, so a tile allocates nothing.
const columnLow = new Int32Array(256), columnHigh = new Int32Array(256), columnPart = new Float64Array(256);
const rowLow = new Int32Array(256), rowHigh = new Int32Array(256), rowPart = new Float64Array(256);
/** The masks of the four data pixels of a cell that hold the band, by level. */
const held = new Uint8Array(EVERY);

/**
 * One axis of a part of `parts` parts of a run tile from data pixel `start`. Up to 2 parts, `low` is
 * the nearest data pixel; with more, the tile edge repeats its pixels.
 */
function axis(low: Int32Array, high: Int32Array, part: Float64Array, start: number, parts: number) {
    for (let s = 0; s < 256; s++) {
        const at = start + (2 * s + 1) / parts, t = parts <= 2 ? Math.floor(at) : Math.min(RUN_TILE - 1, Math.max(0, at - 0.5));
        low[s] = Math.floor(t);
        high[s] = Math.min(RUN_TILE - 1, low[s] + 1);
        part[s] = t - low[s];
    }
}

/** Bits 1, 2, 4 and 8 for the four data pixels of a cell. */
const bits = (a: boolean, b: boolean, c: boolean, d: boolean) => (a ? 1 : 0) | (b ? 2 : 0) | (c ? 4 : 0) | (d ? 8 : 0);
/** The bilinear weight of the pixels of `mask`. */
const weightOf = (mask: number, w0: number, w1: number, w2: number, w3: number) => (mask & 1 ? w0 : 0) + (mask & 2 ? w1 : 0) + (mask & 4 ? w2 : 0) + (mask & 8 ? w3 : 0);

/**
 * Writes the classes of a 256 px map tile on `band` as `palette` words; missing `levels` are no data.
 * The map tile is part (`x`, `y`) of `parts` × `parts` parts of the run tiles. Up to 2 parts, a screen
 * pixel takes the nearest data pixel, so 1 part shows the tile at half size. With more parts,
 * a level holds the band at a screen pixel where the pixels of the four nearest whose run holds it
 * have at least half of the bilinear weight, and the hatch is where pixels with no data have more
 * than half. So class borders are smooth, and they move only between data pixels of different classes.
 */
export function paintTile(words: Uint32Array, levels: readonly Runs[] | undefined, band: number, parts: number, x: number, y: number, palette: Uint32Array) {
    const draw = (px: number, py: number, value: number) => {
        // Diagonal lines every 8 px line up across tile edges.
        if (value && (value !== UNKNOWN || ((px + py) & 7) < 2)) words[py * 256 + px] = palette[value];
    };
    if (!levels) {
        for (let i = 0; i < 256 * 256; i++) draw(i & 255, i >> 8, UNKNOWN);
        return;
    }
    axis(columnLow, columnHigh, columnPart, x * RUN_TILE / parts, parts);
    axis(rowLow, rowHigh, rowPart, y * RUN_TILE / parts, parts);
    if (parts <= 2) {
        for (let py = 0; py < 256; py++) for (let px = 0; px < 256; px++) draw(px, py, levelClass(levels, rowLow[py] * RUN_TILE + columnLow[px], band));
        return;
    }
    const half = levels[HALF].first;
    // A cell is a block of screen pixels between the same four data pixels.
    for (let top = 0, bottom = 0; top < 256; top = bottom) {
        while (bottom < 256 && rowLow[bottom] === rowLow[top]) bottom++;
        const north = rowLow[top] * RUN_TILE, south = rowHigh[top] * RUN_TILE;
        for (let left = 0, right = 0; left < 256; left = right) {
            while (right < 256 && columnLow[right] === columnLow[left]) right++;
            const n0 = north + columnLow[left], n1 = north + columnHigh[left], n2 = south + columnLow[left], n3 = south + columnHigh[left];
            const missing = bits(half[n0] === NO_DATA, half[n1] === NO_DATA, half[n2] === NO_DATA, half[n3] === NO_DATA);
            // A level without a pixel that holds the band has no weight in the cell, so the levels above it do not count.
            let count = 0, full = missing === 0;
            if (missing !== 15) {
                for (; count < levels.length; count++) {
                    const runs = levels[count], mask = bits(holds(runs, n0, band), holds(runs, n1, band), holds(runs, n2, band), holds(runs, n3, band));
                    if (!mask) break;
                    held[count] = mask;
                    if (mask !== 15) full = false;
                }
            }
            if (missing === 15 || full) {
                const value = missing === 15 ? UNKNOWN : count;
                for (let py = top; py < bottom; py++) for (let px = left; px < right; px++) draw(px, py, value);
                continue;
            }
            for (let py = top; py < bottom; py++) {
                const fr = rowPart[py];
                for (let px = left; px < right; px++) {
                    const fc = columnPart[px], w0 = (1 - fr) * (1 - fc), w1 = (1 - fr) * fc, w2 = fr * (1 - fc), w3 = fr * fc;
                    let value = 0;
                    if (weightOf(missing, w0, w1, w2, w3) > 0.5) value = UNKNOWN;
                    else while (value < count && weightOf(held[value], w0, w1, w2, w3) >= 0.5) value++;
                    draw(px, py, value);
                }
            }
        }
    }
}

/** The WebP images of a run tile, one after the other: each is 8 + the RIFF size in bytes 4 to 7 long. */
export function runImages(body: Uint8Array<ArrayBuffer>): Uint8Array<ArrayBuffer>[] {
    const view = new DataView(body.buffer, body.byteOffset, body.byteLength), images: Uint8Array<ArrayBuffer>[] = [];
    for (let at = 0; at < body.length;) {
        const end = at + 8 > body.length ? Infinity : at + 8 + view.getUint32(at + 4, true);
        if (end > body.length) throw new Error('A snow tile ends inside an image.');
        images.push(body.subarray(at, end));
        at = end;
    }
    return images;
}

/**
 * Per season, the route melts out on the latest melt-out of its points and snows in on the earliest
 * onset. The route is clear on day d ⇔ d > melt or d < onset. Null marks a season without data.
 */
function routeSeasons(p: Planar): ({ melt: number; onset: number } | null)[] {
    return Array.from({ length: p.seasons }, (_, s) => {
        let melt = -1, onset = DAYS, known = false;
        for (let i = 0; i < p.size; i++) {
            const a = p.data[2 * s * p.size + i], b = p.data[(2 * s + 1) * p.size + i];
            if (a === NO_DATA || b === NO_DATA) continue;
            known = true;
            if (a === NO_SNOW) continue;
            if (a === WHOLE_SEASON) return { melt: DAYS, onset: -1 };
            melt = Math.max(melt, b);
            onset = Math.min(onset, a);
        }
        return known ? { melt, onset } : null;
    });
}
const routeCache = new WeakMap<Planar, ReturnType<typeof routeSeasons>>();
const seasonsOf = (p: Planar) => routeCache.get(p) ?? routeCache.set(p, routeSeasons(p)).get(p)!;

const years = (n: number) => `${n} ${n === 1 ? 'year' : 'years'}`;

/** The year slider of a route: per day, the share of seasons in which some part of the route has snow. */
export function snowYear(p: Planar | null, date: string, theme: Theme): Grid {
    const grid = (label: string, cells: Uint8Array): Grid => ({ label, columns: COLUMNS, rows: [{ label: '', cells }], fills: snowClasses(theme) });
    if (!p) return grid('Plan a route to see when the whole route is snow-free.', new Uint8Array(COLUMNS).fill(255));
    const seasons = seasonsOf(p).filter(season => season !== null);
    const clearOn = (day: number) => seasons.filter(({ melt, onset }) => day > melt || day < onset).length;
    const cells = Uint8Array.from(columns, ({ index }) => shareClass(seasons.length - clearOn(index), seasons.length));
    return grid(seasons.length ? `Whole route snow-free on ${dateLabel(date)}: ${clearOn(dateDay(date))} of ${years(seasons.length)}` : 'No snow data along the route', cells);
}

/** Calendar years (rows, newest last) × days at item `i`: 0 clear, 1 snow, 2 no data. */
export function snowChart(p: Planar, i: number, firstSeason: number, date: string, theme: Theme): Chart {
    const [snow, known] = snowSeasons(p, i, dateDay(date));
    const cell = (s: number, day: number) => {
        if (s < 0 || s >= p.seasons) return 2;
        const onset = p.data[2 * s * p.size + i], melt = p.data[(2 * s + 1) * p.size + i];
        return onset === NO_DATA || melt === NO_DATA ? 2 : onset === WHOLE_SEASON || (onset <= day && day <= melt) ? 1 : 0;
    };
    // The first and last years are half outside the archive: it starts in September and ends in August.
    const rows = Array.from({ length: p.seasons + 1 }, (_, r) => ({
        label: String(firstSeason + r),
        cells: Uint8Array.from(columns, ({ season, index }) => cell(r + season - 2001, index)),
    }));
    const palette = colors[theme];
    const history: Swatch[] = [{ label: 'Snow on the ground', color: palette.history }, noData(theme)];
    return {
        headline: known ? `Snow on ${dateLabel(date)} in ${snow} of ${years(known)}` : 'No snow data here',
        grids: [{ label: '', columns: COLUMNS, rows, fills: [{ label: 'Clear', color: palette.clear }, ...history], legend: { swatches: history } }],
    };
}

/** The labels of the classes by value. */
const LABELS = [
    'No snow in any year', 'Snow in fewer than 1 in 4 years', 'Snow in 1 in 4 to 1 in 2 years', 'Snow in 1 in 2 to 3 in 4 years',
    'Snow in 3 in 4 years or more, but not every year', 'Snow every year',
];

const colors = {
    light: { classes: ['#e6e0cc', '#d3dcdc', '#becfda', '#a9c1d8', '#c9d9e9', '#e8f1fa'], clear: '#efede4', history: '#5f7d91' },
    dark: { classes: ['#3b382b', '#33434e', '#4f6679', '#6a88a3', '#a9bcce', '#e8f0f8'], clear: '#2b2a21', history: '#9fb7c6' },
} as const;

/** Swatches by class value, then no data. The map leaves the first, no snow in any year, clear. */
export function snowClasses(theme: Theme): Swatch[] {
    return [...LABELS.map((label, value) => ({ label, color: colors[theme].classes[value] })), noData(theme)];
}
