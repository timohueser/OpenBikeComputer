import { cumulative, type Coordinate } from './editor';
import type { RouteClosure, RoutingLine } from './routing';

export interface ClosureStretch {
    closures: RouteClosure[];
    coordinates: Coordinate[];
    km: number;
}

/** The stretches of the line where the rider may have no access. The router uses them, because nothing says for sure that the road is closed. */
export function closureStretches(line?: Pick<RoutingLine, 'coordinates' | 'closures'>): ClosureStretch[] {
    const runs: { closures: RouteClosure[]; from: number; to: number }[] = [];
    line?.closures?.forEach((closures, i) => {
        if (!closures) return;
        let last = runs.at(-1);
        if (last?.to !== i) runs.push(last = { closures: [], from: i, to: i });
        last.to = i + 1;
        for (const closure of closures)
            if (!last.closures.some(known => known.kind === closure.kind && known.condition === closure.condition)) last.closures.push(closure);
    });
    return runs.map(({ closures, from, to }) => {
        const coordinates = line!.coordinates.slice(from, to + 1);
        return { closures, coordinates, km: cumulative(coordinates).at(-1)! };
    });
}

// OSM writes `nov-may` as often as `Nov-May`, and `Oct 14th` as often as `Oct 14`.
const dates = (condition: string) => condition.replace(/\s*-\s*/g, '–').replace(/(\d)(st|nd|rd|th)\b/g, '$1')
    .replace(/\b[a-z]/g, letter => letter.toUpperCase());
const listed = (conditions: string[]) => conditions.length ? ` (${[...new Set(conditions)].join(', ')})` : '';
const phrases: Record<RouteClosure['kind'], (conditions: string[]) => string> = {
    permit: () => 'Permit needed',
    limited: groups => `Access for ${groups.join(' or ')} only`,
    seasonal: conditions => `May be closed seasonally${listed(conditions.map(dates))}`,
    conditional: conditions => `May be closed${listed(conditions.map(condition => condition.replace(/\s*-\s*/g, '–')))}`,
    unclear: values => `Access unclear${listed(values)}`,
};

/** One quiet line, such as "Permit needed · May be closed seasonally (Nov–May) · 12.3 km". */
export function closureNote(stretches: ClosureStretch[]): string {
    const km = stretches.reduce((sum, stretch) => sum + stretch.km, 0);
    const conditions = new Map<RouteClosure['kind'], Set<string>>();
    for (const { kind, condition } of stretches.flatMap(stretch => stretch.closures)) {
        const known = conditions.get(kind) ?? conditions.set(kind, new Set()).get(kind)!;
        if (condition) known.add(condition);
    }
    const kinds = (Object.keys(phrases) as RouteClosure['kind'][]).filter(kind => conditions.has(kind));
    const distance = km < 1 ? `${Math.round(km * 1000)} m` : `${km.toFixed(1)} km`;
    return [...kinds.map(kind => phrases[kind]([...conditions.get(kind)!])), distance].join(' · ');
}
