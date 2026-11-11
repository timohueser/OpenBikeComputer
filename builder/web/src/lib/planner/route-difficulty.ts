import { edgeStretches, type Stretch } from './route-closures';
import type { RoutingLine } from './routing';

/** SAC T4, `alpine_hiking`: the hardest grade of the Alpine hiking preset, and the first that needs a note. */
const ALPINE = 4;

/** The stretches of the line on alpine paths. */
export function alpineStretches(line?: Pick<RoutingLine, 'coordinates' | 'edges'>): Stretch[] {
    return line ? edgeStretches(line.coordinates, line.edges.sac_scale, grade => (grade ?? 0) >= ALPINE) : [];
}

export const alpineLabel = `Alpine path (T${ALPINE})`;
