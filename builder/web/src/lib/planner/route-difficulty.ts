import { edgeStretches, noteDistance, type Stretch } from './route-closures';
import type { RoutingLine } from './routing';

/** SAC T4, `alpine_hiking`: the first grade that needs a note. */
const ALPINE = 4;

export interface AlpineStretch extends Stretch {
    /** The hardest SAC grade on the stretch, 4 to 6. */
    grade: number;
}

/** The stretches of the line on alpine paths. */
export function alpineStretches(line?: Pick<RoutingLine, 'coordinates' | 'edges'>): AlpineStretch[] {
    const grades = line?.edges.sac_scale ?? [];
    return edgeStretches(line?.coordinates ?? [], grades, grade => (grade ?? 0) >= ALPINE)
        .map(({ from, to, coordinates, km }) => ({ coordinates, km, grade: Math.max(...grades.slice(from, to).map(grade => grade ?? 0)) }));
}

/** One quiet line, such as "2.1 km alpine path (T5): exposed, sure-footed hikers only", with the hardest grade. */
export function alpineNote(stretches: AlpineStretch[]): string {
    const km = stretches.reduce((sum, stretch) => sum + stretch.km, 0);
    const grade = Math.max(ALPINE, ...stretches.map(stretch => stretch.grade));
    return `${noteDistance(km)} alpine path (T${grade}): exposed, sure-footed hikers only`;
}
