// The Weather layer of the climate archive: daytime highs or the wet-day share on the map; highs,
// rain and night lows along a route and at a point.
import RainWeeks from '../../../components/planner/RainWeeks.svelte';
import { RAIN_DRIER, RAIN_UNKNOWN, RAIN_WETTER, atElevation, rainClass, rainRatios, read, temperatureAt, weekMonth, wetDaysOf7, WEEKS, YEARS } from './climate';
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
 * Writes a 256 px map tile: each pixel blends the four nearest cell centres. A temperature moves to
 * the pixel's height from each cell's orography with the cell's lapse rate, so valleys and ridges
 * show inside a cell; without `heights` it stays at the blended orography. Less than half the weight
 * on cells with data leaves the pixel clear, so the data ends at the cell edges.
 */
export function paintWeather(words: Uint32Array, cells: { cols: Float64Array; rows: Float64Array }, block: CellBlock, heights: Float32Array | undefined, variable: Variable, palette: Uint32Array) {
    const { value, lapse, orography, cols, rows } = block;
    for (let py = 0; py < 256; py++) {
        const fr = cells.rows[py] - block.row, r0 = Math.floor(fr), wy = fr - r0;
        for (let px = 0; px < 256; px++) {
            const fc = cells.cols[px] - block.col, c0 = Math.floor(fc), wx = fc - c0;
            const height = variable === 'temperature' ? heights?.[py * 256 + px] ?? NaN : NaN;
            let sum = 0, weight = 0;
            for (let k = 0; k < 4; k++) {
                const r = r0 + (k >> 1), c = c0 + (k & 1);
                if (r < 0 || c < 0 || r >= rows || c >= cols) continue;
                const i = r * cols + c;
                if (Number.isNaN(value[i])) continue;
                const w = (k >> 1 ? wy : 1 - wy) * (k & 1 ? wx : 1 - wx);
                weight += w;
                // Each cell's own lapse rate moves its temperature to the pixel height.
                sum += w * (Number.isNaN(height) ? value[i] : atElevation(value[i], lapse[i], orography[i], height));
            }
            words[py * 256 + px] = weight < 0.5 ? 0 : word(variable, palette, sum / weight);
        }
    }
}

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

const oneDecimal = (value: number) => value.toLocaleString('en-GB', { minimumFractionDigits: 1, maximumFractionDigits: 1 });

/** The year slider of a route: per week, the mean high, wet days and night low along the route. */
export function weatherYear(samples: Samples | null, date: string, theme: Theme): Grid {
    const fills = [...temperatureFills(theme), ...wetFills(theme)];
    const empty = new Uint8Array(WEEKS).fill(255);
    const grid = (label: string, high: Uint8Array, low: Uint8Array, rain: Uint8Array): Grid => ({
        label, columns: WEEKS, fills,
        rows: [{ label: 'High', cells: high }, { label: 'Low', cells: low }, { label: 'Rain', cells: rain }],
    });
    if (!samples) return grid('Plan a route to see its weather through the year.', empty, empty, empty);
    const rows = weatherRows(samples.overview, samples.km, samples.elevation);
    const week = weekOf(date);
    const highs = minMax(sampleHighs(samples, date));
    if (!highs) return grid('No weather data along the route', empty, empty, empty);
    const lows = minMax(Float32Array.from(samples.overview, (ref, i) => ref ? temperatureAt(ref.tile, 'tmin', week, ref.index, samples.elevation[i]) : NaN));
    const label = `On the route, ${weekLabel(date)}: highs ${range(...highs)} · lows ${lows ? range(...lows) : 'unknown'} · rain on ${oneDecimal(rows.rain[week])} of 7 days`;
    return grid(label, Uint8Array.from(rows.high, temperatureClass), Uint8Array.from(rows.low, temperatureClass), Uint8Array.from(rows.rain, days => Number.isNaN(days) ? 255 : WET_BASE + wetClass(days)));
}

const leap = (year: number) => year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
/** Days of a week of the spec: week 51 also holds the last one or two days of the year. */
const daysOf = (week: number, year: number) => week < WEEKS - 1 ? 7 : leap(year) ? 9 : 8;

/** Wet days of 7 in each week of the years at a detail cell: the mean, and the spread without the extreme years. */
export function rainWeeks(ref: CellRef, firstYear: number): { mean: Float32Array; low: Float32Array; high: Float32Array } {
    const mean = new Float32Array(WEEKS), low = new Float32Array(WEEKS), high = new Float32Array(WEEKS);
    for (let week = 0; week < WEEKS; week++) {
        const years = Array.from({ length: YEARS }, (_, year) => 7 * read(ref.tile, 'wet_days', year * WEEKS + week, ref.index) / daysOf(week, firstYear + year));
        const known = years.filter(v => !Number.isNaN(v));
        mean[week] = known.length ? known.reduce((a, b) => a + b, 0) / known.length : NaN;
        [low[week], high[week]] = spread(years) ?? [NaN, NaN];
    }
    return { mean, low, high };
}

/** The years at one sample for the map variable: highs and lows, or wet days through the year. */
export function weatherChart(samples: Samples, i: number, firstYear: number, date: string, theme: Theme, variable: Variable): Chart {
    const ref = samples.detail(i), height = samples.elevation[i];
    if (ref === undefined) return { headline: 'Loading the years at this point…', grids: [] };
    if (!ref) return { headline: 'No weather data here', grids: [] };
    const week = weekOf(date);
    if (variable === 'rain') {
        const weeks = rainWeeks(ref, firstYear);
        if (Number.isNaN(weeks.mean[week])) return { headline: 'No rain data here', grids: [] };
        const most = Math.round(weeks.low[week]) === Math.round(weeks.high[week]) ? `${Math.round(weeks.low[week])}` : `${Math.round(weeks.low[week])}–${Math.round(weeks.high[week])}`;
        // The rain amount of the week against the mean week of the cell, over the years.
        const ratios = rainRatios(Float32Array.from({ length: YEARS * WEEKS }, (_, slot) => read(ref.tile, 'rain', slot, ref.index)), firstYear);
        const ofWeek = Array.from({ length: YEARS }, (_, year) => ratios[year * WEEKS + week]).filter(v => !Number.isNaN(v));
        const amount = ofWeek.length ? rainClass(ofWeek.reduce((a, b) => a + b, 0) / ofWeek.length) : RAIN_UNKNOWN;
        return {
            headline: `${weekLabel(date)}: rain on about ${Math.round(weeks.mean[week])} of 7 days (${most} in most years)`,
            grids: [],
            extra: { component: RainWeeks, props: { ...weeks, week, colors: { line: rampColor('rain', theme, 5), band: rampColor('rain', theme, theme === 'dark' ? 3 : 1.5) } } },
            note: amount === RAIN_WETTER ? 'Usually more rain than in an average week here.' : amount === RAIN_DRIER ? 'Usually less rain than in an average week here.' : undefined,
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

