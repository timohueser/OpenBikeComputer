import { edgeStretches, type Stretch } from './route-closures';
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
        .map(({ from, to, coordinates, km, at }) => ({ coordinates, km, at, grade: Math.max(...grades.slice(from, to).map(grade => grade ?? 0)) }));
}

/** Such as "Alpine path (T5)", with the hardest grade of the stretch. */
export const alpineLabel = (stretch: AlpineStretch) => `Alpine path (T${stretch.grade})`;
