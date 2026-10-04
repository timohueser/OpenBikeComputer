import type { Map } from 'maplibre-gl';
import type { Component } from 'svelte';
import type { Coordinate } from '../map-types';

export type Theme = 'light' | 'dark';

/** The data archives of a region; each data layer reads one. */
export type Archive = 'snow' | 'climate' | 'sun';

export interface Swatch {
    label: string;
    color: string;
    hatch?: boolean;
}

/** A map symbol in the legend: a stroke path in a 24 px box. */
export interface Mark { label: string; path: string; width: number }

/** Discrete classes, or a continuous scale through evenly spaced stops; `marks` explain map symbols. */
export type Legend = ({ swatches: Swatch[] } | { scale: { color: string; label: string }[] }) & { marks?: Mark[] };

/** Rows of cells over the calendar year in `columns` steps: 183 steps of two days, or 52 weeks. */
export interface Grid {
    label: string;
    columns: number;
    /** `values` has a short value for each column, shown beside the label for the marked column. */
    rows: { label: string; cells: Uint8Array; values?: string[] }[];
    /** A cell value indexes `fills`; 255 leaves the cell empty. */
    fills: Swatch[];
    legend?: Legend;
}

/** The years at one point: one grid per variable, and an optional view beside them, such as a wind rose. */
export interface Chart {
    headline: string;
    grids: Grid[];
    note?: string;
    extra?: { component: Component<any>; props: Record<string, unknown> };
}

/** What a layer samples: a route with the heights of its profile, or one map point without a height. */
export interface Line {
    coordinates: Coordinate[];
    elevation: (number | null)[];
}

export interface View { date: string; theme: Theme }

/** A map layer with a history at every point. The map shows one data layer at a time. */
export interface DataLayer<Samples = unknown> {
    id: string;
    label: string;
    /** A PlannerIcon name. */
    icon: string;
    description: string;
    /** A limit of the data, beside the attribution. */
    caveat: string;
    /** Attribution and resolution, once the data has loaded. */
    readonly source: string;
    readonly error: string;
    /** A choice of the map variable in the date bar; the map redraws when `value` changes. */
    readonly time?: { value: string; timezone: string; detail?: string };
    /** Resample when a layer depends on the selected instant. */
    sampleKey?(date: string): string;
    readonly variable?: { label: string; options: { value: string; label: string }[]; value: string };
    /** The legend of the map. */
    legend(theme: Theme): Legend;
    /** Installs the layer when it is first shown; a hidden layer requests nothing. */
    sync(map: Map, view: View & { shown: boolean }): void;
    /** The data at each coordinate of a line. */
    sample(line: Line, signal: AbortSignal, view?: View): Promise<Samples>;
    /** The strip under the profile; a layer without it has no strip. Each sample has a value that indexes `fills`. */
    readonly strip?: {
        /** The strip title; the layer label without it. */
        label?: string;
        hint?: string;
        legend?(theme: Theme): Legend;
        fills(theme: Theme): Swatch[];
        values(samples: Samples, date: string): Uint8Array;
        /** A strip coloured over its own range instead of a legend: the range as text, and labels at stretches, such as the warmest. */
        scaled?(samples: Samples, date: string): { range: string; marks: { index: number; label: string }[] } | null;
    };
    /** The years at sample `i`. */
    chart(samples: Samples, i: number, view: View): Chart;
    /** The year slider of a route: its label line and one row per variable; empty rows without samples. */
    year?(samples: Samples | null, view: View): Grid;
    /** A short label for each overnight stop, at sample `index` on the night of `date`; '' for none. A layer without it labels no stops. */
    nights?(samples: Samples, stops: { index: number; date: string }[]): string[];
}

/** The layers whose archive the region has, in list order. */
export function availableLayers(entries: readonly { archive: Archive; create: (url: string) => DataLayer }[], urls: Partial<Record<Archive, string>>): DataLayer[] {
    return entries.flatMap(({ archive, create }) => urls[archive] ? [create(urls[archive]!)] : []);
}

const DAY_MS = 86_400_000;
const JANUARY = Date.UTC(2001, 0, 1);

/** Days per column; the last column takes the rest of the year. */
const step = (columns: number) => Math.round(365 / columns);

/** The first day of a calendar column in a year without a leap day. */
export function columnDay(column: number, columns: number): Date {
    return new Date(JANUARY + step(columns) * column * DAY_MS);
}

/** The first column of each month. */
export function monthColumns(columns: number): number[] {
    return Array.from({ length: columns }, (_, i) => i).filter(i => i === 0 || columnDay(i, columns).getUTCMonth() !== columnDay(i - 1, columns).getUTCMonth());
}

/** "15 Jun" */
export function dateLabel(date: Date | string): string {
    return new Date(date).toLocaleDateString('en-GB', { day: 'numeric', month: 'short', timeZone: 'UTC' });
}

/**
 * The week of a date as specs/planner-climate-tiles.md counts it: days 7w to 7w + 6 of its own year
 * from 1 January, and week 51 also holds the last one or two days.
 */
export function weekOf(date: string): number {
    const [year, month, day] = date.split('-').map(Number);
    return Math.min(51, Math.floor((Date.UTC(year, month - 1, day) - Date.UTC(year, 0, 1)) / DAY_MS / 7));
}

/** 52 columns are the weeks of `weekOf`; in other steps a leap day shares the column of 28 February. */
export function dateColumn(date: string, columns: number): number {
    if (columns === 52) return weekOf(date);
    const [, month, day] = date.split('-').map(Number);
    return Math.min(columns - 1, Math.floor((Date.UTC(2001, month - 1, month === 2 ? Math.min(day, 28) : day) - JANUARY) / DAY_MS / step(columns)));
}

export function columnDate(column: number, year: number, columns: number): string {
    if (columns === 52) return new Date(Date.UTC(year, 0, 1 + 7 * column)).toISOString().slice(0, 10);
    const day = columnDay(column, columns);
    return new Date(Date.UTC(year, day.getUTCMonth(), day.getUTCDate())).toISOString().slice(0, 10);
}

export function addDays(date: string, days: number): string {
    const [year, month, day] = date.split('-').map(Number);
    return new Date(Date.UTC(year, month - 1, day + days)).toISOString().slice(0, 10);
}

/** The same day `months` later, or the last day of a shorter month. */
export function addMonths(date: string, months: number): string {
    const [year, month, day] = date.split('-').map(Number);
    const last = new Date(Date.UTC(year, month + months, 0)).getUTCDate();
    return new Date(Date.UTC(year, month - 1 + months, Math.min(day, last))).toISOString().slice(0, 10);
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
