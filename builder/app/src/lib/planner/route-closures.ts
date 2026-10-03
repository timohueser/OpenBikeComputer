import { cumulative, type Coordinate } from './editor';
import type { RoutingLine } from './routing';

export interface ClosureStretch {
    conditions: string[];
    coordinates: Coordinate[];
    km: number;
}

/** The stretches of the line on a seasonal closure. The router uses them, because only the date of the ride decides. */
export function closureStretches(line?: Pick<RoutingLine, 'coordinates' | 'closures'>): ClosureStretch[] {
    const runs: { conditions: string[]; from: number; to: number }[] = [];
    line?.closures?.forEach((condition, i) => {
        if (!condition) return;
        const last = runs.at(-1);
        if (last?.to !== i) runs.push({ conditions: [condition], from: i, to: i + 1 });
        else {
            last.to = i + 1;
            if (!last.conditions.includes(condition)) last.conditions.push(condition);
        }
    });
    return runs.map(({ conditions, from, to }) => {
        const coordinates = line!.coordinates.slice(from, to + 1);
        return { conditions, coordinates, km: cumulative(coordinates).at(-1)! };
    });
}

/** One quiet line, such as "2.3 km may be closed seasonally (Nov–May)". OSM writes `nov-may` as often as `Nov-May`. */
export function closureNote(stretches: ClosureStretch[]): string {
    const km = stretches.reduce((sum, stretch) => sum + stretch.km, 0);
    const conditions = new Set(stretches.flatMap(stretch => stretch.conditions)
        .map(condition => condition.replace(/\s*-\s*/g, '–').replace(/\b[a-z]/g, letter => letter.toUpperCase())));
    const distance = km < 1 ? `${Math.round(km * 1000)} m` : `${km.toFixed(1)} km`;
    return `${distance} may be closed seasonally (${[...conditions].join(', ')})`;
}
