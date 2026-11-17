import { cumulative } from './geo';
import type { RoutingLine, Surface } from './routing';

export const surfaceBarColors: Record<Surface, string> = { Paved: '#536674', Compacted: '#7d8b63', Gravel: '#ba9c5c', Dirt: '#a36d49', Rough: '#875f72', Unknown: '#b8b5ac' };

/** A stretch of one surface and one pushing state, as route progress. */
export interface SurfaceRun { surface: Surface; pushing: boolean | null; from: number; to: number }

export function surfaceRuns(line?: Pick<RoutingLine, 'coordinates' | 'edges'>): SurfaceRun[] {
    const lengths = cumulative(line?.coordinates ?? []);
    const total = lengths.at(-1) ?? 0;
    const runs: SurfaceRun[] = [];
    if (!total) return runs;
    for (let i = 1; i < lengths.length; i++) {
        const surface = line?.edges.surfaces?.[i - 1] ?? 'Unknown';
        const pushing = line?.edges.pushing?.[i - 1] ?? null;
        const from = lengths[i - 1] / total, to = lengths[i] / total;
        if (to <= from) continue;
        const last = runs.at(-1);
        if (last?.surface === surface && last.pushing === pushing) last.to = to;
        else runs.push({ surface, pushing, from, to });
    }
    return runs;
}

export function surfaceWindow(runs: SurfaceRun[], from: number, to: number): SurfaceRun[] {
    return runs.filter(run => run.to > from && run.from < to).map(run => ({
        ...run, from: Math.max(from, run.from), to: Math.min(to, run.to),
    }));
}

export interface SurfaceShare { surface: Surface; percent: number }

/** The share of each surface of the runs in whole percent, the largest first and Unknown last. The largest-remainder
 * method rounds the shares, so they add to 100: each share rounds down, and the shares with the largest fractions take
 * the points that remain. A surface under one percent can have 0. */
export function surfaceShares(runs: Pick<SurfaceRun, 'surface' | 'from' | 'to'>[]): SurfaceShare[] {
    const lengths = new Map<Surface, number>();
    for (const run of runs) lengths.set(run.surface, (lengths.get(run.surface) ?? 0) + run.to - run.from);
    const total = [...lengths.values()].reduce((sum, length) => sum + length, 0);
    const shares = [...lengths].map(([surface, length]) => ({ surface, exact: length / total * 100, percent: Math.floor(length / total * 100) }))
        .sort((a, b) => Number(a.surface === 'Unknown') - Number(b.surface === 'Unknown') || b.exact - a.exact);
    const rest = 100 - shares.reduce((sum, share) => sum + share.percent, 0);
    for (const share of [...shares].sort((a, b) => b.exact % 1 - a.exact % 1).slice(0, rest)) share.percent++;
    return shares.map(({ surface, percent }) => ({ surface, percent }));
}
