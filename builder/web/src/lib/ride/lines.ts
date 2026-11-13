/** The lines of a recorded ride on its distance axis: smoothed, thinned to one value for each pixel column, and tied to
 * the summary line of the ride. */
import type { FitSample } from '../core/fit';
import { metres } from '../library/summary';
import { cumulative, firstIndex, type Coordinate } from '../planner/geo';
import { zoneOf, type Zone } from '../settings/zones';

/** The metres of each sample from the start; with no `hops`, with no step to a segment start. */
function along(samples: FitSample[], hops = false): number[] {
    const m = [0];
    for (let i = 1; i < samples.length; i++) m.push(m[i - 1] + (samples[i].segmentStart && !hops ? 0 : metres(samples[i - 1], samples[i])));
    return m;
}

/** The place of each sample on the distance axis from 0 to 1, and the length of the axis in metres. */
export function distanceAxis(samples: FitSample[]): { xs: number[]; lengthM: number } {
    const m = along(samples), lengthM = m.at(-1) || 1;
    return { xs: m.map(value => value / lengthM), lengthM };
}

/** The speed of the file, else the distance of the step over its time; none for a step of 0 s or to a segment start. */
export function speeds(samples: FitSample[]): (number | undefined)[] {
    return samples.map((sample, i) => {
        if (sample.speedMps !== undefined) return sample.speedMps;
        if (!i || sample.segmentStart) return undefined;
        const seconds = sample.timeS - samples[i - 1].timeS;
        return seconds > 0 ? metres(samples[i - 1], sample) / seconds : undefined;
    });
}

/** The mean of each value with the values within `seconds / 2` of its time, so a pause ends the window. A missing value
 * stays missing. The times of `samples` do not go back. */
export function smooth(samples: FitSample[], values: (number | undefined)[], seconds: number): (number | undefined)[] {
    const half = seconds / 2, sum = [0], count = [0];
    values.forEach((value, i) => { sum.push(sum[i] + (value ?? 0)); count.push(count[i] + (value === undefined ? 0 : 1)); });
    let from = 0, to = 0;
    return values.map((value, i) => {
        const time = samples[i].timeS;
        while (samples[from].timeS < time - half) from++;
        while (to < samples.length && samples[to].timeS <= time + half) to++;
        return value === undefined ? undefined : (sum[to] - sum[from]) / (count[to] - count[from]);
    });
}

/** The mean of the values in each of `width` columns of the axis. A column between two samples, as in a ride with few
 * points, stands on the straight line between its neighbours. A column whose samples have no value is null, and so is a
 * column beside it with no sample. */
export function thin(xs: number[], values: (number | undefined)[], width: number): (number | null)[] {
    const sum = new Float64Array(width), count = new Uint32Array(width), seen = new Uint8Array(width);
    xs.forEach((x, i) => {
        const value = values[i], column = Math.min(width - 1, Math.floor(x * width));
        seen[column] = 1;
        if (value === undefined || !Number.isFinite(value)) return;
        sum[column] += value;
        count[column]++;
    });
    const columns = Array.from(sum, (total, i) => count[i] ? total / count[i] : null);
    let last = -1;
    for (let i = 0; i < width; i++) {
        if (!seen[i]) continue;
        const [from, to] = [last < 0 ? null : columns[last], columns[i]];
        if (from !== null && to !== null) for (let j = last + 1; j < i; j++) columns[j] = from + (to - from) * (j - last) / (i - last);
        last = i;
    }
    return columns;
}

/** The smallest and the largest value, or undefined with none. */
export function extent(values: (number | null | undefined)[]): { low: number; high: number } | undefined {
    let low = Infinity, high = -Infinity;
    for (const value of values) if (value != null) { low = Math.min(low, value); high = Math.max(high, value); }
    return low > high ? undefined : { low, high };
}

/** The range that a line draws in: its extent, and for a flat line a range of 1, so it has a height. It is no number
 * of the ride: `extent` gives those. The columns of `thin` stay inside the range of their values. */
export function range(values: (number | null | undefined)[]): { low: number; high: number } | undefined {
    const found = extent(values);
    return found && { low: found.low, high: found.high > found.low ? found.high : found.low + 1 };
}

/** The stretches of a line by zone: the columns `from` to `to` are in the zone `zone`, an index of `zones`. A column with
 * no value is in no stretch and ends one. */
export function zoneStretches(columns: (number | null)[], zones: Zone[]): { zone: number; from: number; to: number }[] {
    const stretches: { zone: number; from: number; to: number }[] = [];
    columns.forEach((value, i) => {
        if (value === null) return;
        const zone = zoneOf(zones, value), last = stretches.at(-1);
        if (last && last.to === i - 1 && last.zone === zone) last.to = i;
        else stretches.push({ zone, from: i, to: i });
    });
    return stretches;
}

/** The share of the summary line `line`, from 0 to 1, at each sample. Each point of the line is a sample of the ride
 * (`summarize` keeps points), found by a forward search; the end of the line is the last sample at its place, because a
 * ride can pass its end before it ends there. Between two of them a sample stands in proportion to its metres. The line measures the hop across a pause, which the distance axis leaves out, so the metres here hold it. */
export function lineShares(samples: FitSample[], line: Coordinate[]): number[] {
    const km = cumulative(line), total = km.at(-1) || 1, m = along(samples, true);
    // As `summarize` rounds a point.
    const rounded = (micro: number) => Math.round(micro / 1e6 * 1e5) / 1e5;
    const anchors: number[] = [];
    let from = 0;
    line.forEach(([lon, lat], k) => {
        const here = (i: number) => rounded(samples[i].lon) === lon && rounded(samples[i].lat) === lat;
        let i = from;
        if (k < line.length - 1) while (i < samples.length && !here(i)) i++;
        else for (i = samples.length - 1; i > from && !here(i);) i--;
        if (i < samples.length && here(i)) from = i;
        anchors.push(from);
    });
    const shares = Array<number>(samples.length).fill(0);
    for (let k = 0; k + 1 < anchors.length; k++) {
        const a = anchors[k], b = anchors[k + 1], span = m[b] - m[a];
        for (let i = a; i <= b; i++) shares[i] = (km[k] + (span ? (m[i] - m[a]) / span : +(i === b)) * (km[k + 1] - km[k])) / total;
    }
    for (let i = (anchors.at(-1) ?? 0) + 1; i < samples.length; i++) shares[i] = 1;
    return shares;
}

/** The first sample whose place reaches `place`, or the last sample; `places` do not go back (`xs`, `lineShares`). */
export const sampleAt = (places: number[], place: number): number => Math.min(places.length - 1, firstIndex(places.length, i => places[i] >= place));
