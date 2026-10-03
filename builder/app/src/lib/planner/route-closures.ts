import { cumulative, type Coordinate } from './editor';
import type { RoutingLine } from './routing';

export interface ClosureStretch {
    condition: string;
    coordinates: Coordinate[];
    km: number;
}

/** The stretches of the line on a seasonal closure. The router uses them, because only the date of the ride decides. */
export function closureStretches(line?: Pick<RoutingLine, 'coordinates' | 'closures'>): ClosureStretch[] {
    const runs: { condition: string; from: number; to: number }[] = [];
    line?.closures?.forEach((condition, i) => {
        if (!condition) return;
        const last = runs.at(-1);
        if (last?.to === i && last.condition === condition) last.to = i + 1;
        else runs.push({ condition, from: i, to: i + 1 });
    });
    return runs.map(({ condition, from, to }) => {
        const coordinates = line!.coordinates.slice(from, to + 1);
        return { condition, coordinates, km: cumulative(coordinates).at(-1)! };
    });
}

/** One quiet line, such as "2.3 km may be closed seasonally (Nov–May)". */
export function closureNote(stretches: ClosureStretch[]): string {
    const km = stretches.reduce((sum, stretch) => sum + stretch.km, 0);
    const conditions = new Set(stretches.map(stretch => stretch.condition));
    const distance = km < 1 ? `${Math.round(km * 1000)} m` : `${km.toFixed(1)} km`;
    const condition = conditions.size === 1 ? ` (${[...conditions][0].replace(/\s*-\s*/g, '–')})` : '';
    return `${distance} may be closed seasonally${condition}`;
}
