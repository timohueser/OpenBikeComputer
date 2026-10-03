// Year-slider rows of a route: one value per column, the distance-weighted mean of the samples with data.
import type { Coordinate } from '../map-types';
import { MONTHS, SECTORS, WEEKS, headPart, temperatureAt, wetDaysOf7, windRose } from './climate';
import type { CellRef } from './climate-source';

/** The travel direction at each point, in degrees clockwise from north, from its previous to its next point. */
export function lineBearings(line: Coordinate[]): Float32Array {
    return Float32Array.from(line, (_, i) => {
        const [lon0, lat0] = line[Math.max(0, i - 1)], [lon1, lat1] = line[Math.min(line.length - 1, i + 1)];
        const east = (lon1 - lon0) * Math.cos((lat0 + lat1) * Math.PI / 360), north = lat1 - lat0;
        return (Math.atan2(east, north) * 180 / Math.PI + 360) % 360;
    });
}

/** Each sample covers the line halfway to its neighbours. */
function reach(km: ArrayLike<number>, i: number): number {
    return (km[Math.min(i + 1, km.length - 1)] - km[Math.max(i - 1, 0)]) / 2;
}

interface Run<T> { cell: CellRef; weight: number; sum: T }

/**
 * Consecutive samples in one cell share its values, so a row reads each run once. `add` sums what a
 * row needs from each sample with its distance weight; every row is linear in those sums.
 */
function runs<T>(cells: (CellRef | undefined)[], km: ArrayLike<number>, start: () => T, add: (sum: T, i: number, weight: number) => void): Run<T>[] {
    const list: Run<T>[] = [];
    cells.forEach((cell, i) => {
        if (!cell) return;
        let run = list.at(-1);
        if (run?.cell.tile !== cell.tile || run.cell.index !== cell.index) list.push(run = { cell, weight: 0, sum: start() });
        const weight = reach(km, i);
        run.weight += weight;
        add(run.sum, i, weight);
    });
    return list;
}

/** The weighted mean over the runs with a value; NaN when none has one. */
function mean<T>(list: Run<T>[], value: (run: Run<T>) => number): number {
    let sum = 0, weight = 0;
    for (const run of list) {
        const v = value(run);
        if (Number.isNaN(v)) continue;
        sum += v * run.weight;
        weight += run.weight;
    }
    return sum / weight;
}

/**
 * Weather rows per week of overview cells: the daytime high and the night low in °C, and the wet
 * days of 7. With `elevations`, a run is corrected to the mean elevation of its samples, which equals
 * the mean of their corrected temperatures; a run with a sample without elevation is not corrected.
 */
export function weatherRows(cells: (CellRef | undefined)[], km: ArrayLike<number>, elevations?: ArrayLike<number>) {
    const list = runs(cells, km, () => ({ height: 0 }), (sum, i, weight) => { sum.height += weight * (elevations?.[i] ?? NaN); });
    const row = (value: (run: Run<{ height: number }>, week: number) => number) =>
        Float32Array.from({ length: WEEKS }, (_, week) => mean(list, run => value(run, week)));
    const temperature = (plane: 'tmax' | 'tmin') => row(({ cell, weight, sum }, week) => temperatureAt(cell.tile, plane, week, cell.index, sum.height / weight));
    return { high: temperature('tmax'), rain: row(({ cell }, week) => wetDaysOf7(cell.tile, cell.index, week)), low: temperature('tmin') };
}

/** The headwind chance per month of overview cells, from 0 to 1, for the travel bearing at each sample. */
export function windRow(cells: (CellRef | undefined)[], km: ArrayLike<number>, bearings: ArrayLike<number>): Float32Array {
    // Per run and sector: the weighted part of the sector in the headwind window of each sample.
    const list = runs(cells, km, () => new Float64Array(SECTORS), (parts, i, weight) => {
        for (let s = 0; s < SECTORS; s++) parts[s] += weight * headPart(s, bearings[i]);
    });
    return Float32Array.from({ length: MONTHS }, (_, month) => mean(list, ({ cell, weight, sum }) => {
        const rose = windRose(cell.tile, cell.index, month);
        let head = 0;
        for (let s = 0; s < SECTORS; s++) head += rose[s] * sum[s];
        return head / weight;
    }));
}
