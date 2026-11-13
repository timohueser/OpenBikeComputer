/** The lines of a recorded ride on its distance axis or its time axis: smoothed, thinned to one value for each pixel
 * column, and tied to the summary line of the ride; the stops of the ride, and a part of it as a ride of its own. */
import type { FitSample } from '../core/fit';
import { metres, movingSteps } from '../library/summary';
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

/** No line leads to a sample on the time axis after a step of more than GAP_S. A stop is STOP_S long, or has a segment
 * start in it. */
const GAP_S = 60, STOP_S = 300;

/** The time axis of a ride: the time of its first sample, its length in seconds, the place of each sample from 0 to 1
 * by the clock, and `gaps`: the samples after a step of more than a minute, to which no line leads. */
export function timeAxis(samples: FitSample[]): { start: number; seconds: number; xs: number[]; gaps: number[] } {
    const start = samples[0].timeS, seconds = samples.at(-1)!.timeS - start || 1;
    return {
        start, seconds,
        xs: samples.map(sample => (sample.timeS - start) / seconds),
        gaps: samples.flatMap((sample, i) => i && sample.timeS - samples[i - 1].timeS > GAP_S ? [i] : []),
    };
}

/** The stops of a ride: each is a run of steps that do not move (`movingSteps`), with a segment start in it or 5 min
 * or more long. It goes from the sample `first`, the last one before the run, to the sample `last`. */
export function stops(samples: FitSample[]): { first: number; last: number; seconds: number }[] {
    const steps = movingSteps(samples), found = [];
    for (let i = 1; i < samples.length;) {
        if (steps[i]) { i++; continue; }
        let end = i, pause = samples[i].segmentStart;
        while (end + 1 < samples.length && !steps[end + 1]) { end++; pause ||= samples[end].segmentStart; }
        const seconds = samples[end].timeS - samples[i - 1].timeS;
        if (pause || seconds >= STOP_S) found.push({ first: i - 1, last: end, seconds });
        i = end + 1;
    }
    return found;
}

/** A part with no end in a stop: a start in a stop moves to the last sample of the stop, an end to its first one. A
 * part that is in one stop stays. */
export function outOfStops(found: { first: number; last: number }[], [first, last]: [number, number]): [number, number] {
    const inStop = (sample: number) => found.find(stop => sample >= stop.first && sample <= stop.last);
    const [from, to] = [inStop(first)?.last ?? first, inStop(last)?.first ?? last];
    return from <= to ? [from, to] : [first, last];
}

/** The samples `first` to `last` as a ride of its own: the first sample starts a segment, so no step leads into it. */
export function slice(samples: FitSample[], [first, last]: [number, number]): FitSample[] {
    const part = samples.slice(first, last + 1);
    part[0] = { ...part[0], segmentStart: true };
    return part;
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
 * column beside it with no sample. No line leads to a sample of `gaps`: the columns before it stay null. */
export function thin(xs: number[], values: (number | undefined)[], width: number, gaps: number[] = []): (number | null)[] {
    const sum = new Float64Array(width), count = new Uint32Array(width), seen = new Uint8Array(width), cut = new Uint8Array(width);
    const columnOf = (x: number) => Math.min(width - 1, Math.floor(x * width));
    for (const i of gaps) cut[columnOf(xs[i])] = 1;
    xs.forEach((x, i) => {
        const value = values[i], column = columnOf(x);
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
        if (from !== null && to !== null && !cut[i]) for (let j = last + 1; j < i; j++) columns[j] = from + (to - from) * (j - last) / (i - last);
        last = i;
    }
    return columns;
}

/** A stretch of the axis as a view: at least `least` long about its middle, and inside the axis. */
export function fitView([from, to]: [number, number], least: number): [number, number] {
    const span = Math.min(1, Math.max(least, to - from)), start = Math.min(1 - span, Math.max(0, (from + to - span) / 2));
    // A view that reaches the end of the axis ends at 1, with no rounding error.
    return [start, 1 - start - span < 1e-9 ? 1 : start + span];
}

/** The columns of `thin` for the stretch `from` to `to` of the axis. The line reaches each edge of the stretch: there it
 * has the value on the straight line between the two samples around the edge. */
export function viewColumns(xs: number[], values: (number | undefined)[], [from, to]: [number, number], width: number): (number | null)[] {
    const first = sampleAt(xs, from), places: number[] = [], found: (number | undefined)[] = [];
    /** The value at the place `x`, between the sample before `i` and the sample `i`. */
    const between = (i: number, x: number) => {
        const [a, b] = [values[i - 1], values[i]];
        return a === undefined || b === undefined || xs[i] === xs[i - 1] ? undefined : a + (b - a) * (x - xs[i - 1]) / (xs[i] - xs[i - 1]);
    };
    if (first > 0 && xs[first] > from) { places.push(0); found.push(between(first, from)); }
    let i = first;
    for (; i < xs.length && xs[i] <= to; i++) { places.push((xs[i] - from) / (to - from)); found.push(values[i]); }
    if (i > 0 && i < xs.length && xs[i - 1] < to) { places.push(1); found.push(between(i, to)); }
    return thin(places, found, width);
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
