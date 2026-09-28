import { cumulative } from './editor';
import type { RoutingLine, Surface } from './routing';

export function surfaceRuns(line?: RoutingLine) {
    const lengths = cumulative(line?.coordinates ?? []);
    const total = lengths.at(-1) ?? 0;
    const runs: { surface: Surface; from: number; to: number }[] = [];
    const shares = new Map<Surface, number>();
    if (!total) return { runs, shares };
    for (let i = 1; i < lengths.length; i++) {
        const surface = line?.surfaces?.[i - 1] ?? 'Unknown';
        const from = lengths[i - 1] / total, to = lengths[i] / total;
        if (to <= from) continue;
        shares.set(surface, (shares.get(surface) ?? 0) + to - from);
        const last = runs.at(-1);
        if (last?.surface === surface) last.to = to;
        else runs.push({ surface, from, to });
    }
    return { runs, shares };
}

export function surfaceWindow(runs: ReturnType<typeof surfaceRuns>['runs'], from: number, to: number) {
    return runs.filter(run => run.to > from && run.from < to).map(run => ({
        ...run, from: Math.max(from, run.from), to: Math.min(to, run.to),
    }));
}
