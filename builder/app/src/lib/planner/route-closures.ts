import { cumulative, type Coordinate } from './geo';
import type { RouteClosure, RoutingLine } from './routing';

export interface Stretch {
    coordinates: Coordinate[];
    km: number;
}

export interface ClosureStretch extends Stretch {
    closures: RouteClosure[];
}

/** The runs of consecutive edges that `keep` accepts, as edge ranges with their coordinates and length. */
export function edgeStretches<T>(coordinates: Coordinate[], edges: readonly T[] | undefined, keep: (edge: T) => boolean): (Stretch & { from: number; to: number })[] {
    const runs: { from: number; to: number }[] = [];
    edges?.forEach((edge, i) => {
        if (!keep(edge)) return;
        const last = runs.at(-1);
        if (last?.to === i) last.to = i + 1;
        else runs.push({ from: i, to: i + 1 });
    });
    return runs.map(({ from, to }) => {
        const stretch = coordinates.slice(from, to + 1);
        return { from, to, coordinates: stretch, km: cumulative(stretch).at(-1)! };
    });
}

export const noteDistance = (km: number) => km < 1 ? `${Math.round(km * 1000)} m` : `${km.toFixed(1)} km`;

/** The stretches of the line where the rider may have no access. The router uses them, because nothing says for sure that the road is closed. */
export function closureStretches(line?: Pick<RoutingLine, 'coordinates' | 'edges'>): ClosureStretch[] {
    const edges = line?.edges.closures ?? [];
    return edgeStretches(line?.coordinates ?? [], edges, Boolean).map(({ from, to, coordinates, km }) => {
        const closures: RouteClosure[] = [];
        for (const closure of edges.slice(from, to).flatMap(edge => edge ?? []))
            if (!closures.some(known => known.kind === closure.kind && known.condition === closure.condition)) closures.push(closure);
        return { closures, coordinates, km };
    });
}

// OSM writes `nov-may` as often as `Nov-May`, and `Oct 14th` as often as `Oct 14`.
const dates = (condition: string) => condition.replace(/\s*-\s*/g, '–').replace(/(\d)(st|nd|rd|th)\b/g, '$1')
    .replace(/\b[a-z]/g, letter => letter.toUpperCase());
const listed = (conditions: string[]) => conditions.length ? ` (${[...new Set(conditions)].join(', ')})` : '';
const motorGroups = new Set(['psv', 'bus', 'hgv', 'taxi', 'motorcar', 'motor_vehicle']);
const group = (value: string) => motorGroups.has(value) ? 'motor vehicles' : value.replace(/_/g, ' ');
const phrases: Record<RouteClosure['kind'], (conditions: string[]) => string> = {
    permit: () => 'Permit needed',
    private: () => 'Private road',
    farm: () => 'Farm or forest traffic only',
    sidepath: () => 'Use the side path',
    discouraged: () => 'Access discouraged',
    limited: values => `Access for ${[...new Set(values.flatMap(value => value.split(';')).map(group))].join(' or ')} only`,
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
    return [...kinds.map(kind => phrases[kind]([...conditions.get(kind)!])), noteDistance(km)].join(' · ');
}
