// The Weather layer of the climate archive: daytime highs or the wet-day share on the map; highs,
// rain and night lows along a route and at a point.
import type { FeatureCollection, Point } from 'geojson';
import RainWeeks from '../../../components/planner/RainWeeks.svelte';
import type { Coordinate } from '../map-types';
import { atElevation, read, temperatureAt, typicalRain, typicalRainAt, weekMonth, wetDaysOf7, WEEKS, YEARS } from './climate';
import type { CellRef } from './climate-source';
import { weatherRows } from './climate-route';
import { dateLabel, weekOf, type Chart, type Grid, type Legend, type Swatch, type Theme } from './data-layer';

export type Variable = 'temperature' | 'rain';

/** Evenly spaced colour stops from `from` to `to`; values outside take the end colours. */
interface Ramp { from: number; to: number; colors: Record<Theme, string[]>; labels: string[] }
const RAMPS: Record<Variable, Ramp> = {
    // Daytime highs in °C: cold blue, a neutral near 10 °C, warm clay; the dark ramp is mixed toward the dark base.
    temperature: {
        from: -10, to: 30,
        colors: { light: ['#6f8fb3', '#b3c8d9', '#ebe6d2', '#e3b07f', '#c5643d'], dark: ['#3f5f80', '#5f819c', '#8a8664', '#b5844f', '#c8603c'] },
        labels: ['−10', '0', '10', '20', '30 °C'],
    },
    // Wet days of 7: dry cream to deep blue-grey. The scale ends at 6 of 7, so typical weeks get most of the colour range.
    rain: {
        from: 0, to: 6,
        colors: { light: ['#efeadb', '#dce3d6', '#c2d6d4', '#a1c1cb', '#7fa8bf', '#5f8cae', '#46719a'], dark: ['#2d2c22', '#303a32', '#334641', '#365358', '#3b6173', '#44708b', '#507fa2'] },
        labels: ['0', '', '2', '', '4', '', '6 of 7 days'],
    },
};

const rgb = (hex: string) => [1, 3, 5].map(i => parseInt(hex.slice(i, i + 2), 16));
const hex = (channels: number[]) => '#' + channels.map(c => Math.round(c).toString(16).padStart(2, '0')).join('');

/** The colour of a value on the variable's scale, mixed between the two nearest stops. */
export function rampColor(variable: Variable, theme: Theme, value: number): string {
    const { from, to, colors } = RAMPS[variable], stops = colors[theme];
    const at = Math.min(1, Math.max(0, (value - from) / (to - from))) * (stops.length - 1);
    const i = Math.min(stops.length - 2, Math.floor(at)), a = rgb(stops[i]), b = rgb(stops[i + 1]);
    return hex(a.map((c, k) => c + (b[k] - c) * (at - i)));
}

export function mapLegend(variable: Variable, theme: Theme): Legend {
    const { colors, labels } = RAMPS[variable];
    return { scale: colors[theme].map((color, i) => ({ color, label: labels[i] })) };
}

/** 256 ABGR words over the scale of a variable, for ImageData pixels. */
export function rampWords(variable: Variable, theme: Theme): Uint32Array {
    const { from, to } = RAMPS[variable];
    return Uint32Array.from({ length: 256 }, (_, i) => {
        const [r, g, b] = rgb(rampColor(variable, theme, from + (to - from) * i / 255));
        return ((255 << 24) | (b << 16) | (g << 8) | r) >>> 0;
    });
}

/** The word of a value; 0 (transparent) for NaN. */
export function word(variable: Variable, words: Uint32Array, value: number): number {
    if (Number.isNaN(value)) return 0;
    const { from, to } = RAMPS[variable];
    return words[Math.min(255, Math.max(0, Math.round((value - from) / (to - from) * 255)))];
}

const T_LOW = -25, T_HIGH = 40;
/** The no-data class; the temperature classes in 1 °C steps come before it. */
export const NO_TEMPERATURE = T_HIGH - T_LOW + 1;
const WET_STEP = 0.25, WET_CLASSES = 7 / WET_STEP + 1;

export function temperatureClass(celsius: number): number {
    return Number.isNaN(celsius) ? NO_TEMPERATURE : Math.min(T_HIGH, Math.max(T_LOW, Math.round(celsius))) - T_LOW;
}

const noData = (theme: Theme): Swatch => ({ label: 'No data', color: theme === 'dark' ? '#6b685c' : '#b8b5ac', hatch: true });

export function temperatureFills(theme: Theme): Swatch[] {
    return [...Array.from({ length: NO_TEMPERATURE }, (_, i) => ({ label: `${celsius(T_LOW + i)} °C`, color: rampColor('temperature', theme, T_LOW + i) })), noData(theme)];
}

const wetFills = (theme: Theme): Swatch[] =>
    Array.from({ length: WET_CLASSES }, (_, i) => ({ label: `${i * WET_STEP} of 7 days`, color: rampColor('rain', theme, i * WET_STEP) }));
const wetClass = (days: number) => Math.round(Math.min(7, Math.max(0, days)) / WET_STEP);
/** The year slider fills: temperature classes, then wet-day classes from here. */
const WET_BASE = NO_TEMPERATURE + 1;

/** "−3", "12": whole degrees with a true minus sign. */
export const celsius = (value: number) => String(Math.round(value)).replace('-', '−');

/** "3–8 °C", "−4 to 2 °C" or "5 °C". */
export function range(low: number, high: number): string {
    const [a, b] = [celsius(low), celsius(high)];
    return a === b ? `${a} °C` : `${a}${low < -0.5 ? ' to ' : '–'}${b} °C`;
}

/** The spread of yearly values without the lowest and the highest year; null without values. */
export function spread(values: ArrayLike<number>): [number, number] | null {
    const sorted = Array.from(values).filter(v => !Number.isNaN(v)).sort((a, b) => a - b);
    if (!sorted.length) return null;
    const drop = sorted.length >= 5 ? 1 : 0;
    return [sorted[drop], sorted[sorted.length - 1 - drop]];
}

const DAY_MS = 86_400_000;

/** "14–20 May" or "28 Apr – 4 May": the days of the climate week of a date. */
export function weekLabel(date: string): string {
    const year = Number(date.slice(0, 4)), week = weekOf(date);
    const start = new Date(Date.UTC(year, 0, 1 + 7 * week));
    const end = week === WEEKS - 1 ? new Date(Date.UTC(year, 11, 31)) : new Date(start.getTime() + 6 * DAY_MS);
    return start.getUTCMonth() === end.getUTCMonth() ? `${start.getUTCDate()}–${dateLabel(end)}` : `${dateLabel(start)} – ${dateLabel(end)}`;
}

/** A block of global cells around a map tile, with the overview values of one week. */
export interface CellBlock {
    col: number; row: number; cols: number; rows: number;
    /** Temperature at the cell orography, or wet days of 7; NaN where missing. */
    value: Float32Array;
    lapse: Float32Array;
    orography: Float32Array;
}

/** The values of each cell of a block for a week, from its overview cell reference. */
export function cellBlock(col: number, row: number, cols: number, rows: number, cell: (col: number, row: number) => CellRef | undefined, variable: Variable, week: number): CellBlock {
    const n = cols * rows, block = { col, row, cols, rows, value: new Float32Array(n).fill(NaN), lapse: new Float32Array(n), orography: new Float32Array(n) };
    for (let r = 0; r < rows; r++) {
        for (let c = 0; c < cols; c++) {
            const ref = cell(col + c, row + r), k = r * cols + c;
            if (!ref) continue;
            if (variable === 'rain') block.value[k] = wetDaysOf7(ref.tile, ref.index, week);
            else {
                block.value[k] = read(ref.tile, 'tmax', week, ref.index);
                block.lapse[k] = read(ref.tile, 'lapse_tmax', weekMonth(week), ref.index);
                block.orography[k] = read(ref.tile, 'orography', 0, ref.index);
            }
        }
    }
    return block;
}

/** The fractional global cell column and row of each pixel column and row of a 256 px map tile. */
export function tileCells(z: number, x: number, y: number): { cols: Float64Array; rows: Float64Array } {
    const scale = 256 * 2 ** z;
    return {
        cols: Float64Array.from({ length: 256 }, (_, px) => 10 * ((x * 256 + px + 0.5) / scale * 360)),
        rows: Float64Array.from({ length: 256 }, (_, py) => 10 * (90 - Math.atan(Math.sinh(Math.PI * (1 - 2 * (y * 256 + py + 0.5) / scale))) * 180 / Math.PI)),
    };
}

/**
 * The value at fractional cell position (`fr`, `fc`) from the block origin: the four nearest cell
 * centres blended. A temperature moves to `height` from each cell's orography with the cell's lapse
 * rate, so valleys and ridges show inside a cell; a NaN height leaves it at the blended orography.
 * NaN when less than half the weight is on cells with data, so the data ends at the cell edges.
 */
export function blend(block: CellBlock, fr: number, fc: number, height: number): number {
    const { value, lapse, orography, cols, rows } = block;
    const r0 = Math.floor(fr), wy = fr - r0, c0 = Math.floor(fc), wx = fc - c0;
    let sum = 0, weight = 0;
    for (let k = 0; k < 4; k++) {
        const r = r0 + (k >> 1), c = c0 + (k & 1);
        if (r < 0 || c < 0 || r >= rows || c >= cols) continue;
        const i = r * cols + c;
        if (Number.isNaN(value[i])) continue;
        const w = (k >> 1 ? wy : 1 - wy) * (k & 1 ? wx : 1 - wx);
        weight += w;
        sum += w * (Number.isNaN(height) ? value[i] : atElevation(value[i], lapse[i], orography[i], height));
    }
    return weight < 0.5 ? NaN : sum / weight;
}

/** Writes a 256 px map tile: each pixel blends the cells around it, a temperature at the pixel's height in `heights`. */
export function paintWeather(words: Uint32Array, cells: { cols: Float64Array; rows: Float64Array }, block: CellBlock, heights: Float32Array | undefined, variable: Variable, palette: Uint32Array) {
    for (let py = 0; py < 256; py++) {
        const fr = cells.rows[py] - block.row;
        for (let px = 0; px < 256; px++) {
            const height = variable === 'temperature' ? heights?.[py * 256 + px] ?? NaN : NaN;
            words[py * 256 + px] = word(variable, palette, blend(block, fr, cells.cols[px] - block.col, height));
        }
    }
}

/** A basemap place that can carry a label: a town or village, or a named peak with its height. */
export interface MapPlace { name: string; coordinate: Coordinate; peak: boolean; population: number; rank: number; minZoom: number; elevation?: number }

/** A basemap feature as `querySourceFeatures` returns it. */
export interface BasemapFeature { id?: string | number; properties: Record<string, unknown> | null; geometry: { type: string; coordinates?: unknown } }

/** The towns, villages and named peaks from map features, once each: a place repeats in each loaded tile that holds it. */
export function mapPlaces(features: BasemapFeature[]): MapPlace[] {
    const places = new Map<string, MapPlace>();
    for (const { id, properties: p, geometry } of features) {
        if (!p || geometry.type !== 'Point') continue;
        // The name the basemap labels show.
        const name = p['name:en'] ?? p['pgf:name'] ?? p.name, peak = p.kind === 'summit';
        if (typeof name !== 'string' || !(p.kind === 'locality' || peak)) continue;
        const coordinate = p.lon != null && p.lat != null ? [Number(p.lon), Number(p.lat)] as Coordinate : geometry.coordinates as Coordinate;
        places.set(String(id ?? `${name} ${coordinate}`), {
            name, coordinate, peak, population: Number(p.population ?? 0), rank: Number(p.population_rank ?? 0), minZoom: Number(p.min_zoom ?? (peak ? 12 : 0)),
            ...(peak && typeof p.elevation === 'number' ? { elevation: p.elevation } : {}),
        });
    }
    return [...places.values()];
}

/**
 * A named peak ranks with a town of this many people plus its height in metres, so labels go to
 * towns first, then named peaks, then villages: basemap towns have about 5,000 people or more.
 */
const PEAK_POPULATION = 5000;

/** The properties of a map label; `min_zoom` and `population_rank` feed the basemap's town label style. */
export interface PlaceLabel { name: string; peak: boolean; min_zoom: number; population_rank: number; sort: number; value?: string }

/**
 * Map labels of the places: the name, and `values[i]` beside it where known. MapLibre places them in
 * `sort` order and drops each label that collides, so a busy view thins from the smallest places. A
 * peak without a value has no label, because the planner marks peaks itself.
 */
export function placeLabels(places: MapPlace[], values: (string | undefined)[]): FeatureCollection<Point, PlaceLabel> {
    return {
        type: 'FeatureCollection',
        features: places.flatMap((place, i) => place.peak && !values[i] ? [] : [{
            type: 'Feature' as const,
            geometry: { type: 'Point' as const, coordinates: place.coordinate },
            properties: {
                name: place.name, peak: place.peak, min_zoom: place.minZoom, population_rank: place.rank,
                sort: -(place.peak ? PEAK_POPULATION + place.elevation! : place.population),
                ...(values[i] ? { value: values[i] } : {}),
            },
        }]),
    };
}

/** The overview cell at the north-west of a coordinate's four nearest cell centres, and the coordinate's position from it. */
function around([longitude, latitude]: Coordinate): { col: number; row: number; fc: number; fr: number } {
    const fc = 10 * (longitude + 180), fr = 10 * (90 - latitude), col = Math.floor(fc), row = Math.floor(fr);
    return { col, row, fc: fc - col, fr: fr - row };
}

/** The daytime high of a week at a coordinate and height, as the map colours it there. */
export function highAt(coordinate: Coordinate, height: number, cell: (col: number, row: number) => CellRef | undefined, week: number): number {
    const { col, row, fc, fr } = around(coordinate);
    return blend(cellBlock(col, row, 2, 2, cell, 'temperature', week), fr, fc, height);
}

/** The typical rain of a week at a coordinate in mm per 7 days, blended between the cells like the map. */
export function rainAt(coordinate: Coordinate, cell: (col: number, row: number) => CellRef | undefined, week: number, firstYear: number): number {
    const { col, row, fc, fr } = around(coordinate);
    const value = Float32Array.from([[0, 0], [1, 0], [0, 1], [1, 1]], ([c, r]) => {
        const ref = cell(col + c, row + r);
        return ref ? typicalRainAt(ref.tile, ref.index, week, firstYear) : NaN;
    });
    return blend({ col, row, cols: 2, rows: 2, value, lapse: new Float32Array(4), orography: new Float32Array(4) }, fr, fc, NaN);
}

/**
 * Typical rain as a range two 5 mm steps wide, centred on the 5 mm step nearest the value: 13 mm is
 * "10–20", so the value is always at least 2.5 mm inside it. Below 2.5 mm it is "0–5". A map label
 * passes a hyphen as `dash`, because the en dash is in a glyph range the basemap does not load.
 */
export function rainRange(mm: number, dash = '–'): string {
    const high = 5 * Math.round(mm / 5) + 5;
    return `${Math.max(0, high - 10)}${dash}${high}`;
}

/** "18°", or "-3°" with a hyphen: the true minus sign is in a glyph range the basemap does not load. */
export const mapDegrees = (celsius: number) => `${Math.round(celsius)}°`;

/** A route or a point: the overview cell of each sample, its height, and the route kilometres. */
export interface Samples {
    overview: (CellRef | undefined)[];
    /** The detail cell of a sample, loaded on the first read (`detailCell`): undefined while it loads, null without data. */
    detail: (i: number) => CellRef | null | undefined;
    /** Metres; NaN where unknown. */
    elevation: Float32Array;
    km: ArrayLike<number>;
}

/** The daytime high of the week of `date` at each sample, at its height. */
export function sampleHighs({ overview, elevation }: Samples, date: string): Float32Array {
    const week = weekOf(date);
    return Float32Array.from(overview, (ref, i) => ref ? temperatureAt(ref.tile, 'tmax', week, ref.index, elevation[i]) : NaN);
}

const minMax = (values: ArrayLike<number>) => {
    const known = Array.from(values).filter(v => !Number.isNaN(v));
    return known.length ? [Math.min(...known), Math.max(...known)] as const : null;
};

/** Colour steps of the route strip. */
const STRIP_STEPS = 32;

/** The route strip colours: the temperature ramp in even steps, then no data. */
export function stripFills(theme: Theme): Swatch[] {
    const { from, to } = RAMPS.temperature;
    return [...Array.from({ length: STRIP_STEPS }, (_, i) => ({ label: '', color: rampColor('temperature', theme, from + (to - from) * i / (STRIP_STEPS - 1)) })), noData(theme)];
}

/**
 * The narrowest span of the stretched strip ramp in °C, centred between the coldest and the warmest
 * sample. Chosen for this strip: a rider barely notices less than a few degrees, so a route within
 * them keeps near-even colours instead of the full ramp.
 */
const STRIP_MIN_SPAN = 4;

/**
 * The strip class of each high on the ramp stretched from the coldest to the warmest sample, so the
 * strip shows where the route is warmer even where the map colours barely change along it.
 */
export function stripClasses(highs: ArrayLike<number>): Uint8Array {
    const [low, high] = minMax(highs) ?? [0, 0];
    const span = Math.max(high - low, STRIP_MIN_SPAN), from = (low + high - span) / 2;
    return Uint8Array.from(highs, value => Number.isNaN(value) ? STRIP_STEPS : Math.round((value - from) / span * (STRIP_STEPS - 1)));
}

/** The middle sample of the stretch around sample `at` that stays within half a degree of its value. */
function stretchMiddle(highs: ArrayLike<number>, at: number): number {
    const near = (i: number) => Math.abs(highs[i] - highs[at]) <= 0.5;
    let from = at, to = at;
    while (from > 0 && near(from - 1)) from--;
    while (to < highs.length - 1 && near(to + 1)) to++;
    return (from + to) >> 1;
}

/** The range of the highs along the route, and labels at its coldest and warmest stretch; null without data. */
export function stripScale(highs: ArrayLike<number>): { range: string; marks: { index: number; label: string }[] } | null {
    const extremes = minMax(highs);
    if (!extremes) return null;
    const values = Array.from(highs);
    const marks = celsius(extremes[0]) === celsius(extremes[1]) ? []
        : extremes.map(value => ({ index: stretchMiddle(highs, values.indexOf(value)), label: `${celsius(value)}°` }));
    return { range: `${range(...extremes)} along the route`, marks };
}

const oneDecimal = (value: number) => value.toLocaleString('en-GB', { minimumFractionDigits: 1, maximumFractionDigits: 1 });
const degrees = (value: number) => Number.isNaN(value) ? '–' : `${celsius(value)}°`;
const rainValue = (days: number, mm: number) => Number.isNaN(days) ? '–' : `${Math.round(days)}/7 d${Number.isNaN(mm) ? '' : ` · ${rainRange(mm)} mm`}`;

/** The year slider of a route: per week, the mean high, night low, wet days and typical rain along the route. */
export function weatherYear(samples: Samples | null, date: string, theme: Theme, firstYear: number): Grid {
    const fills = [...temperatureFills(theme), ...wetFills(theme)];
    const empty = new Uint8Array(WEEKS).fill(255);
    const grid = (label: string, rows: Grid['rows']): Grid => ({ label, columns: WEEKS, fills, rows });
    const blank = (label: string) => grid(label, ['High', 'Low', 'Rain'].map(row => ({ label: row, cells: empty })));
    if (!samples) return blank('Plan a route to see its weather through the year.');
    const rows = weatherRows(samples.overview, samples.km, samples.elevation);
    const week = weekOf(date);
    const highs = minMax(sampleHighs(samples, date));
    if (!highs) return blank('No weather data along the route');
    const lows = minMax(Float32Array.from(samples.overview, (ref, i) => ref ? temperatureAt(ref.tile, 'tmin', week, ref.index, samples.elevation[i]) : NaN));
    const amount = typicalRain(rows.amount, firstYear);
    const label = `On the route, ${weekLabel(date)}: highs ${range(...highs)} · lows ${lows ? range(...lows) : 'unknown'} · rain on ${oneDecimal(rows.rain[week])} of 7 days${Number.isNaN(amount[week]) ? '' : `, ${rainRange(amount[week])} mm`}`;
    return grid(label, [
        { label: 'High', cells: Uint8Array.from(rows.high, temperatureClass), values: Array.from(rows.high, degrees) },
        { label: 'Low', cells: Uint8Array.from(rows.low, temperatureClass), values: Array.from(rows.low, degrees) },
        { label: 'Rain', cells: Uint8Array.from(rows.rain, days => Number.isNaN(days) ? 255 : WET_BASE + wetClass(days)), values: Array.from(rows.rain, (days, w) => rainValue(days, amount[w])) },
    ]);
}

/** The mean of the values that are not missing; NaN without any. */
function mean(values: number[]): number {
    const known = values.filter(v => !Number.isNaN(v));
    return known.length ? known.reduce((a, b) => a + b, 0) / known.length : NaN;
}

const leap = (year: number) => year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
/** Days of a week of the spec: week 51 also holds the last one or two days of the year. */
const daysOf = (week: number, year: number) => week < WEEKS - 1 ? 7 : leap(year) ? 9 : 8;

/** Wet days of 7 in each week of the years at a detail cell: the mean, and the spread without the extreme years. */
export function rainWeeks(ref: CellRef, firstYear: number): { mean: Float32Array; low: Float32Array; high: Float32Array } {
    const means = new Float32Array(WEEKS), low = new Float32Array(WEEKS), high = new Float32Array(WEEKS);
    for (let week = 0; week < WEEKS; week++) {
        const years = Array.from({ length: YEARS }, (_, year) => 7 * read(ref.tile, 'wet_days', year * WEEKS + week, ref.index) / daysOf(week, firstYear + year));
        means[week] = mean(years);
        [low[week], high[week]] = spread(years) ?? [NaN, NaN];
    }
    return { mean: means, low, high };
}

/** The years at one sample for the map variable: highs and lows, or wet days through the year with the typical rain. */
export function weatherChart(samples: Samples, i: number, firstYear: number, date: string, theme: Theme, variable: Variable): Chart {
    const ref = samples.detail(i), height = samples.elevation[i];
    if (ref === undefined) return { headline: 'Loading the years at this point…', grids: [] };
    if (!ref) return { headline: 'No weather data here', grids: [] };
    const week = weekOf(date);
    if (variable === 'rain') {
        const weeks = rainWeeks(ref, firstYear);
        if (Number.isNaN(weeks.mean[week])) return { headline: 'No rain data here', grids: [] };
        const most = Math.round(weeks.low[week]) === Math.round(weeks.high[week]) ? `${Math.round(weeks.low[week])}` : `${Math.round(weeks.low[week])}–${Math.round(weeks.high[week])}`;
        // The detail cell is the overview cell, so its ten-year mean totals are the overview's.
        const totals = Float32Array.from({ length: WEEKS }, (_, w) => mean(Array.from({ length: YEARS }, (_, year) => read(ref.tile, 'rain', year * WEEKS + w, ref.index))));
        const mm = typicalRain(totals, firstYear)[week];
        return {
            headline: `${weekLabel(date)}: rain on about ${Math.round(weeks.mean[week])} of 7 days (${most} in most years)${Number.isNaN(mm) ? '' : ` · ${rainRange(mm)} mm a week`}`,
            grids: [],
            extra: { component: RainWeeks, props: { ...weeks, week, colors: { line: rampColor('rain', theme, 5), band: rampColor('rain', theme, theme === 'dark' ? 3 : 1.5) } } },
        };
    }
    const temperatures = (plane: 'tmax' | 'tmin') => Float32Array.from({ length: YEARS * WEEKS }, (_, slot) => temperatureAt(ref.tile, plane, slot, ref.index, height));
    const high = temperatures('tmax'), low = temperatures('tmin');
    const ofWeek = (values: Float32Array) => spread(Array.from({ length: YEARS }, (_, year) => values[year * WEEKS + week]));
    const highs = ofWeek(high), lows = ofWeek(low);
    const parts = [highs && `highs ${range(...highs)}`, lows && `night lows ${range(...lows)}`].filter(Boolean);
    const years = (values: Float32Array, label: string, legend?: Legend): Grid => ({
        label, columns: WEEKS, fills: temperatureFills(theme), legend,
        rows: Array.from({ length: YEARS }, (_, year) => ({ label: String(firstYear + year), cells: Uint8Array.from({ length: WEEKS }, (_, w) => temperatureClass(values[year * WEEKS + w])) })),
    });
    return {
        headline: parts.length ? `${weekLabel(date)}: ${parts.join(', ')}` : 'No weather data here',
        grids: [years(high, 'Daytime high'), years(low, 'Night low', mapLegend('temperature', theme))],
        note: 'Valleys can be colder on clear nights.',
    };
}

/** "Night 3–8 °C" at each stop: the night lows of the stop's week over the years, at its height. */
export function nightLabels(samples: Samples, stops: { index: number; date: string }[]): string[] {
    return stops.map(({ index, date }) => {
        const ref = samples.detail(index), week = weekOf(date);
        if (!ref) return '';
        const lows = spread(Array.from({ length: YEARS }, (_, year) => temperatureAt(ref.tile, 'tmin', year * WEEKS + week, ref.index, samples.elevation[index])));
        return lows ? `Night ${range(...lows)}` : '';
    });
}

