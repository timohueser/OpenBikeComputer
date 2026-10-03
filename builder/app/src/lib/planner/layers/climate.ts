// Climate tiles decoded per specs/planner-climate-tiles.md. Both levels hold the same 0.1° ERA5-Land
// cells: the detail level (zoom 9) has each week of ten years, the overview level (zoom 8) has the
// ten-year means and the monthly wind rose. A missing value reads as NaN.
import type { Coordinate } from '../map-types';

export const YEARS = 10, WEEKS = 52, MONTHS = 12, SECTORS = 16;
export const OVERVIEW = 8, DETAIL = 9;
export type Level = typeof OVERVIEW | typeof DETAIL;

export type Plane = 'orography' | 'lapse_tmax' | 'lapse_tmin' | 'rose' | 'wet_share' | 'wet_days' | 'rain' | 'tmax' | 'tmin' | 'wind';
export type WeekPlane = 'wet_share' | 'rain' | 'tmax' | 'tmin' | 'wind';

interface Code { bytes: 1 | 2; signed: boolean; missing: number; value: (code: number) => number }
const u8 = (value: (code: number) => number): Code => ({ bytes: 1, signed: false, missing: 255, value });
const i8 = (step: number): Code => ({ bytes: 1, signed: true, missing: -128, value: code => step * code });
const CODES: Record<Plane, Code> = {
    wet_days: u8(code => code),
    wet_share: u8(code => code),
    rain: u8(code => code <= 100 ? code : 100 + 5 * (code - 100)),
    tmax: i8(0.5),
    tmin: i8(0.5),
    wind: u8(code => 0.5 * code),
    orography: { bytes: 2, signed: true, missing: -32768, value: code => code },
    lapse_tmax: i8(0.1),
    lapse_tmin: i8(0.1),
    rose: u8(code => 0.5 * code),
};

interface Layout { cols: number; rows: number; bytes: number; planes: Partial<Record<Plane, { start: number; count: number }>> }

function layout(cols: number, rows: number, planes: [Plane, number][]): Layout {
    const result: Layout = { cols, rows, bytes: 0, planes: {} };
    for (const [name, count] of planes) {
        result.planes[name] = { start: result.bytes, count };
        result.bytes += count * cols * rows * CODES[name].bytes;
    }
    return result;
}

const TERRAIN: [Plane, number][] = [['orography', 1], ['lapse_tmax', MONTHS], ['lapse_tmin', MONTHS]];
const LAYOUTS: Record<Level, Layout> = {
    [OVERVIEW]: layout(24, 16, [...TERRAIN, ['rose', MONTHS * SECTORS], ['wet_share', WEEKS], ['rain', WEEKS], ['tmax', WEEKS], ['tmin', WEEKS], ['wind', WEEKS]]),
    [DETAIL]: layout(12, 8, [...TERRAIN, ...(['wet_days', 'rain', 'tmax', 'tmin', 'wind'] as const).map((name): [Plane, number] => [name, YEARS * WEEKS])]),
};

export interface ClimateMeta { firstYear: number; years: number; wetDayMm: number; attribution: string }

export function climateMeta(json: Record<string, unknown>): ClimateMeta {
    const meta = { firstYear: Number(json.first_year), years: Number(json.years), wetDayMm: Number(json.wet_day_mm), attribution: String(json.attribution ?? '') };
    if (!Number.isInteger(meta.firstYear) || meta.years !== YEARS || !(meta.wetDayMm > 0)) throw new Error('The climate archive has no valid metadata.');
    return meta;
}

/** A decompressed tile body. Cell `index` counts in row order from the north-west cell of the tile. */
export interface ClimateTile { level: Level; x: number; y: number; body: Uint8Array }

export function climateTile(level: Level, x: number, y: number, body: Uint8Array): ClimateTile {
    if (body.length !== LAYOUTS[level].bytes) throw new Error(`Climate tile ${level}/${x}/${y} has ${body.length} bytes.`);
    return { level, x, y, body };
}

export const tileCells = (level: Level) => LAYOUTS[level].cols * LAYOUTS[level].rows;

/** The value of `plane` at `index` (week, month or sector slot) and cell; NaN where missing. */
export function read(tile: ClimateTile, plane: Plane, index: number, cell: number): number {
    const { planes, cols, rows } = LAYOUTS[tile.level];
    const code = CODES[plane], at = planes[plane];
    if (!at) throw new Error(`Level ${tile.level} has no ${plane} plane.`);
    const offset = at.start + (index * cols * rows + cell) * code.bytes, b = tile.body;
    const raw = code.bytes === 2 ? (b[offset] | (b[offset + 1] << 8)) << 16 >> 16 : code.signed ? b[offset] << 24 >> 24 : b[offset];
    return raw === code.missing ? NaN : code.value(raw);
}

/** A global grid cell: column 0 is centred on 180° W, row 0 on 90° N. */
export interface Cell { col: number; row: number }

export function cellAt([lon, lat]: Coordinate): Cell {
    return { col: ((Math.floor(10 * (lon + 180) + 0.5) % 3600) + 3600) % 3600, row: Math.floor(10 * (90 - lat) + 0.5) };
}

export function cellCentre({ col, row }: Cell): Coordinate {
    return [-180 + 0.1 * col, 90 - 0.1 * row];
}

/** The tile of a level that holds a cell, and the cell index in it. */
export function locate(level: Level, { col, row }: Cell): { x: number; y: number; index: number } {
    const { cols, rows } = LAYOUTS[level];
    return { x: Math.floor(col / cols), y: Math.floor(row / rows), index: (row % rows) * cols + (col % cols) };
}

const DAY_MS = 86_400_000;

/** Week 51 also holds the last one or two days of the year. */
export function weekOf(date: string): number {
    const [year, month, day] = date.split('-').map(Number);
    return Math.min(WEEKS - 1, Math.floor((Date.UTC(year, month - 1, day) - Date.UTC(year, 0, 1)) / DAY_MS / 7));
}

/** The month (0 = January) of the middle day of a week. */
export function weekMonth(week: number): number {
    return new Date(Date.UTC(2001, 0, 1 + 7 * week + 3)).getUTCMonth();
}

/** Overview values of a week plane at every cell of the tile. */
export function weekValues(tile: ClimateTile, plane: WeekPlane, week: number): Float32Array {
    return Float32Array.from({ length: tileCells(tile.level) }, (_, cell) => read(tile, plane, week, cell));
}

export interface CellHistory { wetDays: Float32Array; rain: Float32Array; tmax: Float32Array; tmin: Float32Array; wind: Float32Array }

/** Every year × week of a detail cell, at index 52 × year + week; temperatures at the cell orography. */
export function cellHistory(tile: ClimateTile, cell: number): CellHistory {
    const plane = (name: Plane) => Float32Array.from({ length: YEARS * WEEKS }, (_, i) => read(tile, name, i, cell));
    return { wetDays: plane('wet_days'), rain: plane('rain'), tmax: plane('tmax'), tmin: plane('tmin'), wind: plane('wind') };
}

/** A temperature at the cell orography, moved to `elevation` with a lapse rate in K/km. */
export function atElevation(temperature: number, lapse: number, orography: number, elevation: number): number {
    return temperature + lapse * (elevation - orography) / 1000;
}

/**
 * The mean daily high or low of week slot `index` (a week, or 52 × year + week on the detail level),
 * at `elevation` with the lapse rate of the week's month; at the cell orography without an elevation.
 */
export function temperatureAt(tile: ClimateTile, plane: 'tmax' | 'tmin', index: number, cell: number, elevation?: number): number {
    const value = read(tile, plane, index, cell);
    if (elevation === undefined || Number.isNaN(elevation)) return value;
    return atElevation(value, read(tile, plane === 'tmax' ? 'lapse_tmax' : 'lapse_tmin', weekMonth(index % WEEKS), cell), read(tile, 'orography', 0, cell), elevation);
}

/** The ten-year mean of wet days in a week, as days of 7 (2.4 of 7); week 51 counts at the same rate. */
export function wetDaysOf7(tile: ClimateTile, cell: number, week: number): number {
    return 7 * read(tile, 'wet_share', week, cell) / 100;
}

/** Days in a week of the mean year: week 51 has 8, or 9 in a leap year. */
const weekDays = (week: number) => week === WEEKS - 1 ? 8.25 : 7;

/**
 * Rain of each week as a multiple of the mean week of the same series: (rain ÷ days of the week) ÷
 * (total rain ÷ total days), over the weeks that are not missing. Slot i is week i mod 52, so the 52
 * overview means and the 520 detail weeks of a cell both work. A series without rain gives NaN.
 */
export function rainRatios(rain: ArrayLike<number>): Float32Array {
    let total = 0, days = 0;
    for (let i = 0; i < rain.length; i++) {
        if (Number.isNaN(rain[i])) continue;
        total += rain[i];
        days += weekDays(i % WEEKS);
    }
    const daily = total / days;
    return Float32Array.from({ length: rain.length }, (_, i) => daily > 0 ? rain[i] / weekDays(i % WEEKS) / daily : NaN);
}

export const RAIN_DRIER = 0, RAIN_TYPICAL = 1, RAIN_WETTER = 2, RAIN_UNKNOWN = 3;
/**
 * A week is wetter above this multiple of the mean week and drier below its inverse. Chosen for this
 * layer: a quarter more rain is a difference a rider notices, and the ten-year means of Baden-Württemberg
 * put about a fifth of the cell weeks on each side.
 */
export const WETTER_RATIO = 1.25;

export function rainClass(ratio: number): number {
    if (Number.isNaN(ratio)) return RAIN_UNKNOWN;
    return ratio > WETTER_RATIO ? RAIN_WETTER : ratio < 1 / WETTER_RATIO ? RAIN_DRIER : RAIN_TYPICAL;
}

/**
 * The daytime wind rose of a month as shares that sum to 1, by the sector the wind blows towards:
 * sector s is centred on 22.5 s° clockwise from north. The archive counts the sector the wind comes
 * from, 8 sectors round. NaN in every sector where the cell has no rose.
 */
export function windRose(tile: ClimateTile, cell: number, month: number): Float32Array {
    const rose = Float32Array.from({ length: SECTORS }, (_, s) => read(tile, 'rose', month * SECTORS + (s + SECTORS / 2) % SECTORS, cell));
    const sum = rose.reduce((a, b) => a + b, 0);
    return rose.map(share => share / sum);
}

/** Shares of the three sectors centred on each sector. */
function windows(rose: ArrayLike<number>): number[] {
    return Array.from({ length: SECTORS }, (_, s) => rose[(s + SECTORS - 1) % SECTORS] + rose[s] + rose[(s + 1) % SECTORS]);
}

/**
 * Two directions dominate when the busiest window opposite the main one (within ±45° of straight
 * opposite) has at least BIMODAL_RATIO of the main window's share and the two windows together hold
 * at least BIMODAL_SHARE. Chosen on the Baden-Württemberg archive: they mark the Lake Constance shore
 * in most months and the northern Upper Rhine valley in spring, and no month at the Feldberg, on the
 * Swabian Alb or at Stuttgart, where the westerlies only have a weaker north-east return.
 */
export const BIMODAL_RATIO = 0.6, BIMODAL_SHARE = 0.65;
const OPPOSITE_SPAN = 2;

/**
 * `towards` is the centre of the busiest three-sector window and `steadiness` its share (3/16 for an
 * even rose, 1 for one direction). `opposite` is the centre of the second dominant window when two
 * roughly opposite directions dominate. Undefined for a missing rose.
 */
export function windMode(rose: ArrayLike<number>): { towards: number; steadiness: number; opposite?: number } | undefined {
    if (Number.isNaN(rose[0])) return undefined;
    const share = windows(rose);
    // The quantised rose often ties two windows (up to rounding); the busier centre sector wins.
    let towards = 0;
    for (let s = 1; s < SECTORS; s++) {
        const gain = share[s] - share[towards];
        if (gain > 1e-9 || (gain > -1e-9 && rose[s] > rose[towards])) towards = s;
    }
    let opposite = (towards + SECTORS / 2) % SECTORS;
    for (let d = -OPPOSITE_SPAN; d <= OPPOSITE_SPAN; d++) {
        const s = (towards + SECTORS / 2 + d + SECTORS) % SECTORS;
        if (share[s] > share[opposite]) opposite = s;
    }
    const bimodal = share[opposite] >= BIMODAL_RATIO * share[towards] && share[opposite] + share[towards] >= BIMODAL_SHARE;
    return { towards, steadiness: share[towards], ...(bimodal ? { opposite } : {}) };
}

/** Half the width of the headwind and of the tailwind window in degrees; crosswind is the rest. */
export const HEADWIND_HALF_ANGLE = 45;
const SECTOR_WIDTH = 360 / SECTORS;

/**
 * The part of the 22.5° of a towards-sector in the headwind window of a bearing: the wind comes from
 * within ±45° of straight ahead. The tailwind window is the headwind window of the opposite bearing.
 * Partial sectors make the chances change smoothly with the bearing.
 */
export function headPart(sector: number, bearing: number): number {
    const along = Math.abs((((sector * SECTOR_WIDTH - bearing) % 360) + 540) % 360 - 180);
    return Math.min(1, Math.max(0, (HEADWIND_HALF_ANGLE + SECTOR_WIDTH / 2 - (180 - along)) / SECTOR_WIDTH));
}

/** Chances of headwind, crosswind and tailwind for a travel bearing; they sum to 1. */
export function windChance(rose: ArrayLike<number>, bearing: number): { head: number; cross: number; tail: number } {
    let head = 0, tail = 0;
    for (let s = 0; s < SECTORS; s++) {
        head += rose[s] * headPart(s, bearing);
        tail += rose[s] * headPart(s, bearing + 180);
    }
    return { head, cross: 1 - head - tail, tail };
}
