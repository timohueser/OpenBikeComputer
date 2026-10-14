import type { RoutingLine } from './routing';
import { cumulative, firstIndex } from './geo';

export type ProfileSample = { progress: number; height: number | null };

// A route is never changed after it is built, so each route's samples are computed once.
const samplesByRoute = new WeakMap<object, readonly ProfileSample[]>();

export function profileSamples(line?: Pick<RoutingLine, 'coordinates' | 'elevation'>): readonly ProfileSample[] {
    if (!line) return [];
    const known = samplesByRoute.get(line);
    if (known) return known;
    const distance = cumulative(line.coordinates);
    const total = distance.at(-1) || 1;
    const samples = line.elevation.map((height, i) => ({ progress: distance[i] / total, height }));
    samplesByRoute.set(line, samples);
    return samples;
}

/** Whole hundreds of metres that hold every known height of the sample lists; 0 to 100 m without one. */
export function heightRange(...lists: (readonly ProfileSample[])[]): { low: number; high: number } {
    let min = Infinity, max = -Infinity;
    for (const list of lists) for (const { height } of list) if (height !== null) { min = Math.min(min, height); max = Math.max(max, height); }
    if (min === Infinity) return { low: 0, high: 100 };
    const low = Math.floor(min / 100) * 100;
    return { low, high: Math.max(low + 100, Math.ceil(max / 100) * 100) };
}

/** SVG paths of a profile in a 1000 × 105 box between the heights `low` and `high`: one line, and the area below it, for
 * each run of known heights. The route spans `width` of the box. */
export function profilePaths(samples: readonly ProfileSample[], low: number, high: number, width = 1): { line: string; area: string } {
    const runs: { line: string; area: string }[] = [];
    let path = '', first = 0, last = 0;
    function close() {
        if (path) runs.push({ line: path, area: `${path}L${last} 105L${first} 105Z` });
        path = '';
    }
    for (const sample of samples) {
        if (sample.height === null) { close(); continue; }
        last = sample.progress * width * 1000;
        if (!path) first = last;
        path += `${path ? 'L' : 'M'}${last} ${105 - (sample.height - low) / (high - low) * 100}`;
    }
    close();
    return { line: runs.map(run => run.line).join(' '), area: runs.map(run => run.area).join(' ') };
}

function profileChange(from: number, to: number, line: RoutingLine | undefined, direction: number): number {
    const samples = profileSamples(line);
    let ascent = 0;
    const last = Math.min(samples.length - 1, sampleIndex(samples, to));
    for (let i = Math.max(1, sampleIndex(samples, from)); i <= last; i++) {
        const a = samples[i - 1], b = samples[i];
        if (a.height === null || b.height === null || b.progress <= a.progress) continue;
        const share = Math.max(0, Math.min(b.progress, to) - Math.max(a.progress, from)) / (b.progress - a.progress);
        ascent += Math.max(0, (b.height - a.height) * direction) * share;
    }
    return Math.round(ascent);
}

export function profileAscent(from = 0, to = 1, line?: RoutingLine): number {
    return profileChange(from, to, line, 1);
}

export function profileDescent(from = 0, to = 1, line?: RoutingLine): number {
    return profileChange(from, to, line, -1);
}

/** The index of the first sample at or past `progress`, or the sample count. */
export function sampleIndex(samples: readonly ProfileSample[], progress: number): number {
    return firstIndex(samples.length, i => samples[i].progress >= progress);
}

export function profileHeightAt(progress: number, samples: readonly ProfileSample[]): number | null {
    const i = sampleIndex(samples, progress);
    if (i === samples.length) return samples.at(-1)?.height ?? null;
    if (i === 0) return samples[0].height;
    const a = samples[i - 1], b = samples[i];
    if (a.height === null || b.height === null) return null;
    return a.height + (b.height - a.height) * (progress - a.progress) / (b.progress - a.progress || 1);
}
