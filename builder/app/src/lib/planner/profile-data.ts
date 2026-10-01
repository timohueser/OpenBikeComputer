import type { RoutingLine } from './routing';
import { cumulative } from './editor';

export function profileSamples(line?: RoutingLine): { progress: number; height: number | null }[] {
    if (!line) return [];
    const distance = cumulative(line.coordinates);
    const total = distance.at(-1) || 1;
    return line.elevation.map((height, i) => ({ progress: distance[i] / total, height }));
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

export function profileHeightAt(progress: number, line?: RoutingLine): number | null {
    const samples = profileSamples(line);
    const i = samples.findIndex(p => p.progress >= progress);
    if (i < 0) return samples.at(-1)?.height ?? null;
    if (i === 0) return samples[0].height;
    const a = samples[i - 1], b = samples[i];
    if (a.height === null || b.height === null) return null;
    return a.height + (b.height - a.height) * (progress - a.progress) / (b.progress - a.progress || 1);
}
