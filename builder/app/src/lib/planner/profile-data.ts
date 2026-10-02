import type { RoutingLine } from './routing';
import { cumulative, firstIndex } from './editor';

export type ProfileSample = { progress: number; height: number | null };

// A route is never changed after it is built, so each route's samples are computed once.
const samplesByRoute = new WeakMap<RoutingLine, readonly ProfileSample[]>();

export function profileSamples(line?: RoutingLine): readonly ProfileSample[] {
    if (!line) return [];
    const known = samplesByRoute.get(line);
    if (known) return known;
    const distance = cumulative(line.coordinates);
    const total = distance.at(-1) || 1;
    const samples = line.elevation.map((height, i) => ({ progress: distance[i] / total, height }));
    samplesByRoute.set(line, samples);
    return samples;
}

function profileChange(from: number, to: number, line: RoutingLine | undefined, direction: number): number {
    const samples = profileSamples(line);
    let ascent = 0;
    for (let i = 1; i < samples.length; i++) {
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
