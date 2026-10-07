import { edgeStretches, noteDistance, type Stretch } from './route-closures';
import type { RoutingLine } from './routing';

/** SAC T4, `alpine_hiking`: the hardest grade that the hiking profile uses, and the first that needs a note. */
const ALPINE = 4;

/** The stretches of the line on alpine paths. */
export function alpineStretches(line?: Pick<RoutingLine, 'coordinates' | 'edges'>): Stretch[] {
    return line ? edgeStretches(line.coordinates, line.edges.sac_scale, grade => (grade ?? 0) >= ALPINE) : [];
}

/** One quiet line, such as "2.1 km alpine path (T4): exposed, sure-footed hikers only". */
export function alpineNote(stretches: Stretch[]): string {
    const km = stretches.reduce((sum, stretch) => sum + stretch.km, 0);
    return `${noteDistance(km)} alpine path (T${ALPINE}): exposed, sure-footed hikers only`;
}
