import type { Map } from 'maplibre-gl';
import type { Component } from 'svelte';
import type { Coordinate } from '../map-types';

export type Theme = 'light' | 'dark';

/** The data archives of a region; each data layer reads one. */
export type Archive = 'snow' | 'climate';

export interface Swatch {
    label: string;
    color: string;
    hatch?: boolean;
}

/** Discrete classes, or a continuous scale through evenly spaced stops. */
export type Legend = { swatches: Swatch[] } | { scale: { color: string; label: string }[] };

/** Rows of cells over the calendar year in `columns` steps: 183 steps of two days, or 52 weeks. */
export interface Grid {
    label: string;
    columns: number;
    rows: { label: string; cells: Uint8Array }[];
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
    readonly variable?: { label: string; options: { value: string; label: string }[]; value: string };
    /** The legend of the map. */
    legend(theme: Theme): Legend;
    /** Installs the layer when it is first shown; a hidden layer requests nothing. */
    sync(map: Map, view: View & { shown: boolean }): void;
    /** The data at each coordinate of a line. */
    sample(line: Line, signal: AbortSignal): Promise<Samples>;
    /** The strip under the profile; a layer without it has no strip. Each sample has a value that indexes `fills`. */
    readonly strip?: {
        legend(theme: Theme): Legend;
        fills(theme: Theme): Swatch[];
        values(samples: Samples, date: string): Uint8Array;
    };
    /** The years at sample `i`. */
    chart(samples: Samples, i: number, view: View): Chart;
    /** The year slider of a route: its label line and one row per variable; empty rows without samples. */
    year(samples: Samples | null, view: View): Grid;
}

/** The layers whose archive the region has, in list order. */
export function availableLayers(entries: readonly { archive: Archive; create: (url: string) => DataLayer }[], urls: Record<Archive, string>): DataLayer[] {
    return entries.flatMap(({ archive, create }) => urls[archive] ? [create(urls[archive])] : []);
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

/** A leap day shares the column of 28 February. */
export function dateColumn(date: string, columns: number): number {
    const [, month, day] = date.split('-').map(Number);
    return Math.min(columns - 1, Math.floor((Date.UTC(2001, month - 1, month === 2 ? Math.min(day, 28) : day) - JANUARY) / DAY_MS / step(columns)));
}

export function columnDate(column: number, year: number, columns: number): string {
    const day = columnDay(column, columns);
    return new Date(Date.UTC(year, day.getUTCMonth(), day.getUTCDate())).toISOString().slice(0, 10);
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
