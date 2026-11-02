// The helpers of the junctions' corners (junction-plan.ts makes them): the ways that a road runs on into, its half-width
// at a vertex, and whether a point lies in a corner, where the stamp paints its asphalt and the sidewalk round it.
import { junctionWays, metresPerUnit, ROAD, splitThrough, type Line } from './bins';
import type { Corner } from './junction-plan';
import { gridOf, near, type Grid } from './town';

/** What locates a corner: its arc's centre and its ends in world units, and the half-width of its narrower way. */
type Place = Pick<Corner, 'o' | 'a' | 'b' | 'h'>;

const grids = new WeakMap<Corner[], { arcs: Corner[]; grid: Grid }>();
const paired = new WeakMap<Line[], Map<Line, [Line?, Line?]>>();

/** Per road, the roads that its start and its end run on into: the other way of a through road that OSM splits at the junction, a bridge's or a tunnel's too. */
export function partnersOf(lines: Line[]): Map<Line, [Line?, Line?]> {
    const known = paired.get(lines);
    if (known) return known;
    const partner = new Map<Line, [Line?, Line?]>();
    for (const ways of junctionWays(lines)) {
        const [a, b] = splitThrough(ways) ?? [];
        for (const [way, other] of a ? [[a, b], [b, a]] : []) {
            if (way.line.kind === ROAD) (partner.get(way.line) ?? partner.set(way.line, [undefined, undefined]).get(way.line)!)[way.v === 0 ? 0 : 1] = other.line;
        }
    }
    paired.set(lines, partner);
    return partner;
}

/** A road's half-width at its vertex v with the widening of a bend: halfway to its partner's at an end that runs on into one, so the paint's and the corridor's edges run on across the joint. */
export function halfAt(line: Line, v: number, partners: Map<Line, [Line?, Line?]>): number {
    const other = v === 0 ? partners.get(line)?.[0] : v === line.z.length - 1 ? partners.get(line)?.[1] : undefined;
    return (other ? (line.h + other.h) / 2 : line.h) + (line.plan?.widen[v] ?? 0);
}

/** The rounded corner of a junction plan that a point lies in, if any. */
export function cornerAt(corners: Corner[], x: number, y: number): Corner | undefined {
    let known = grids.get(corners);
    if (!known) {
        const arcs = corners.filter(({ r }) => r > 0);
        grids.set(corners, (known = { arcs, grid: gridOf(arcs.map(boxOf)) }));
    }
    const { arcs, grid } = known;
    let hit: Corner | undefined;
    near(grid, x, y, x, y, 0, k => (hit ??= inCorner(arcs[k], x, y) ? arcs[k] : undefined));
    return hit;
}

/** The box of a corner as inCorner has it: its arc's centre, its ends moved its reach onto the roads, and where the edges through those meet. */
export function boxOf({ o, a, b, h }: Place): number[] {
    const reach = h / metresPerUnit(o[1]), out = (end: number[]) => {
        const e = [end[0] - o[0], end[1] - o[1]], k = 1 + reach / Math.hypot(e[0], e[1]);
        return [o[0] + k * e[0], o[1] + k * e[1]];
    };
    const [pa, pb] = [out(a), out(b)], da = [o[1] - pa[1], pa[0] - o[0]], eb = [pb[0] - o[0], pb[1] - o[1]];
    const t = ((pb[0] - pa[0]) * eb[0] + (pb[1] - pa[1]) * eb[1]) / (da[0] * eb[0] + da[1] * eb[1]);
    const xs = [o[0], pa[0], pb[0], pa[0] + t * da[0]], ys = [o[1], pa[1], pb[1], pa[1] + t * da[1]];
    return [Math.min(...xs), Math.min(...ys), Math.max(...xs), Math.max(...ys)];
}

/**
 * Whether a point lies in a corner: between the ends of its arc seen from its centre, and off both roads or
 * less than the narrower road's half-width onto them, where the arc is the nearest kerb. So no road's edge
 * that the arc replaces paints where a road bends inside the corner.
 */
export function inCorner({ o, a, b, h }: Place, x: number, y: number, onto = h): boolean {
    const ea = [a[0] - o[0], a[1] - o[1]], eb = [b[0] - o[0], b[1] - o[1]], e = [x - o[0], y - o[1]], reach = onto / metresPerUnit(o[1]);
    const t = Math.atan2(ea[0] * e[1] - ea[1] * e[0], ea[0] * e[0] + ea[1] * e[1]) / Math.atan2(ea[0] * eb[1] - ea[1] * eb[0], ea[0] * eb[0] + ea[1] * eb[1]);
    const past = (end: number[]) => ((e[0] - end[0]) * end[0] + (e[1] - end[1]) * end[1]) / Math.hypot(end[0], end[1]);
    return t >= 0 && t <= 1 && past(ea) <= reach && past(eb) <= reach;
}
