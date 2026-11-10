import { cumulative, type Coordinate } from './geo';
import type { RouteClosure, RoutingLine } from './routing';

export interface Stretch {
    coordinates: Coordinate[];
    km: number;
    /** The km along the route where the stretch starts. */
    at: number;
}

type Warning = Exclude<RouteClosure['kind'], 'sidepath'>;
type WarningClosure = RouteClosure & { kind: Warning };

export interface ClosureStretch extends Stretch {
    closures: WarningClosure[];
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
    const along = cumulative(coordinates);
    return runs.map(({ from, to }) => {
        const stretch = coordinates.slice(from, to + 1);
        return { from, to, coordinates: stretch, km: cumulative(stretch).at(-1)!, at: along[from] };
    });
}

export const noteDistance = (km: number) => km < 1 ? `${Math.round(km * 1000)} m` : `${km.toFixed(1)} km`;

// A signed cycle path beside the road is a choice for the router, not a warning for the rider.
const warns = (closure: RouteClosure): closure is WarningClosure => closure.kind !== 'sidepath';

/** The stretches of the line where the rider may have no access. The router uses them, because nothing says for sure that the road is closed. */
export function closureStretches(line?: Pick<RoutingLine, 'coordinates' | 'edges'>): ClosureStretch[] {
    const edges = line?.edges.closures ?? [];
    return edgeStretches(line?.coordinates ?? [], edges, edge => !!edge?.some(warns)).map(({ from, to, coordinates, km, at }) => {
        const closures: WarningClosure[] = [];
        for (const closure of edges.slice(from, to).flatMap(edge => edge ?? []).filter(warns))
            if (!closures.some(known => known.kind === closure.kind && known.condition === closure.condition)) closures.push(closure);
        return { closures, coordinates, km, at };
    });
}

// OSM writes `nov-may` as often as `Nov-May`, and `Oct 14th` as often as `Oct 14`.
const dates = (condition: string) => condition.replace(/\s*-\s*/g, '–').replace(/(\d)(st|nd|rd|th)\b/g, '$1')
    .replace(/\b[a-z]/g, letter => letter.toUpperCase());
const listed = (conditions: string[]) => conditions.length ? ` (${[...new Set(conditions)].join(', ')})` : '';
const motorGroups = new Set(['psv', 'bus', 'hgv', 'taxi', 'motorcar', 'motor_vehicle']);
const group = (value: string) => motorGroups.has(value) ? 'motor vehicles' : value.replace(/_/g, ' ');
const groups = (values: string[]) => [...new Set(values.flatMap(value => value.split(';')).map(group))];
const phrases: Record<Warning, (conditions: string[]) => string> = {
    permit: () => 'Permit needed',
    private: () => 'Private road',
    farm: () => 'Farm or forest traffic only',
    discouraged: () => 'Access discouraged',
    limited: values => groups(values).join() === 'customers' ? 'Customers only' : `Only for ${groups(values).join(' or ')}`,
    seasonal: conditions => `May be closed seasonally${listed(conditions.map(dates))}`,
    conditional: conditions => `May be closed${listed(conditions.map(condition => condition.replace(/\s*-\s*/g, '–')))}`,
    unclear: values => `Access unclear${listed(values)}`,
};

/** The phrases of one stretch, such as "Permit needed · May be closed seasonally (Nov–May)". */
export function closureLabel(stretch: ClosureStretch): string {
    const conditions = new Map<Warning, Set<string>>();
    for (const { kind, condition } of stretch.closures) {
        const known = conditions.get(kind) ?? conditions.set(kind, new Set()).get(kind)!;
        if (condition) known.add(condition);
    }
    const kinds = (Object.keys(phrases) as Warning[]).filter(kind => conditions.has(kind));
    return kinds.map(kind => phrases[kind]([...conditions.get(kind)!])).join(' · ');
}
