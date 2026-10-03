import type { Map } from 'maplibre-gl';
import type { Coordinate } from '../map-types';

export type Theme = 'light' | 'dark';

export interface Swatch {
    label: string;
    color: string;
    hatch?: boolean;
    /** Drawn along lines only; the map leaves it clear. */
    lineOnly?: boolean;
}

/** Rows of cells, one column per two days of the calendar year. A cell value indexes `swatches`; 255 leaves it empty. */
export interface SeasonGrid {
    rows: { label: string; cells: Uint8Array }[];
    /** The column of the layer date. */
    marker: number;
    swatches: Swatch[];
}

export interface Inspection { headline: string; grid: SeasonGrid }
export interface LineStats { headline: string; detail: string; year: { label: string; grid: SeasonGrid } }

/** A map layer with a history at every point. Snow is the only one. */
export interface DataLayer<Samples = unknown> {
    id: string;
    label: string;
    description: string;
    /** A limit of the data, beside the attribution. */
    caveat: string;
    /** Shown at a point under trees. */
    treeNote: string;
    /** Attribution and resolution, once the data has loaded. */
    readonly source: string;
    readonly error: string;
    /** Swatches by class value. */
    swatches(theme: Theme): Swatch[];
    /** Installs the layer when it is first shown; a hidden layer requests nothing. */
    sync(map: Map, view: { shown: boolean; date: string; theme: Theme }): void;
    /** The data at each coordinate of a line, or at one point. */
    sample(line: Coordinate[], signal: AbortSignal): Promise<Samples>;
    /** The class of each sample on a date. */
    classes(samples: Samples, date: string): Uint8Array;
    /** The season × day grid at sample `i`. */
    inspect(samples: Samples, i: number, date: string, theme: Theme): Inspection;
    /** `km` holds the route distance of each sample. */
    stats(samples: Samples, km: ArrayLike<number>, date: string, theme: Theme): LineStats;
}

export const COLUMNS = 183;
const DAY_MS = 86_400_000;
const JANUARY = Date.UTC(2001, 0, 1);

/** The first day of each calendar column in a year without a leap day. */
export const columnDays = Array.from({ length: COLUMNS }, (_, column) => new Date(JANUARY + 2 * column * DAY_MS));
export const monthColumns = columnDays.flatMap((day, i) => i === 0 || day.getUTCMonth() !== columnDays[i - 1].getUTCMonth() ? [i] : []);

/** "15 Jun" */
export function dateLabel(date: Date | string): string {
    return new Date(date).toLocaleDateString('en-GB', { day: 'numeric', month: 'short', timeZone: 'UTC' });
}

/** A leap day shares the column of 28 February. */
export function dateColumn(date: string): number {
    const [, month, day] = date.split('-').map(Number);
    return Math.min(COLUMNS - 1, Math.floor((Date.UTC(2001, month - 1, month === 2 ? Math.min(day, 28) : day) - JANUARY) / DAY_MS / 2));
}

/** The same day `months` later, or the last day of a shorter month. */
export function addMonths(date: string, months: number): string {
    const [year, month, day] = date.split('-').map(Number);
    const last = new Date(Date.UTC(year, month + months, 0)).getUTCDate();
    return new Date(Date.UTC(year, month - 1 + months, Math.min(day, last))).toISOString().slice(0, 10);
}

export function columnDate(column: number, year: number): string {
    const day = columnDays[column];
    return new Date(Date.UTC(year, day.getUTCMonth(), day.getUTCDate())).toISOString().slice(0, 10);
}

/** Each sample covers the line halfway to its neighbours. */
export function reach(at: ArrayLike<number>, i: number): number {
    return (at[Math.min(i + 1, at.length - 1)] - at[Math.max(i - 1, 0)]) / 2;
}

/** Runs of equal values along a line, as progress from 0 to 1. */
export function valueRuns(values: ArrayLike<number>, progress: ArrayLike<number>): { value: number; from: number; to: number }[] {
    const runs: { value: number; from: number; to: number }[] = [];
    for (let i = 0; i < values.length; i++) {
        const from = i ? (progress[i - 1] + progress[i]) / 2 : 0;
        const to = i < values.length - 1 ? (progress[i] + progress[i + 1]) / 2 : 1;
        const last = runs.at(-1);
        if (last?.value === values[i]) last.to = to;
        else if (to > from) runs.push({ value: values[i], from, to });
    }
    return runs;
}
