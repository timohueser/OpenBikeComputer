// What a road's plan paints and lowers: a zebra where OSM tags one, a give-way line across the mouth of a minor road,
// and the lowered kerb under a zebra, a path's end or a driveway. looks.wgsl draws the paint from what each vertex carries.
import { metresPerUnit, ROAD } from '../terrain/bins';
import type { Way } from './arms';
import { classOf, LOOK, lookOf, surfaceName } from './section';
import type { Station } from './stations';
import type { Run } from './sweep';

/** A zebra's half-length along the road. */
const ZEBRA_HALF_M = 2;
/** A crossing node lies on its way's line, and the fit moves a line less than this off its OSM nodes. */
const NODE_REACH_M = 2.5;
/** Two ways that hold a node at their shared point lie this near alike, and run on across it this straight. */
const SAME_FOOT_M = 0.05, ON_COS = 0.7;
/** The give-way line lies this far back from the through road's edge, this wide, and its faces reach this far: the line, then the triangle. As GIVE_WAY_M of looks.wgsl. */
const GIVE_WAY_BACK_M = 0.4, GIVE_WAY_M = 0.3, GIVE_WAY_REACH_M = 4.2;
/** A marked stretch needs this many metres of its way past it; a triangle needs this many. */
const LINE_ROOM_M = 1, TRIANGLE_ROOM_M = 2;
/** The classes that a minor road gives way to, best first; a road gives way unless it is of the same class or that class's link. */
const MAIN = ['motorway', 'motorway_link', 'trunk', 'trunk_link', 'primary', 'primary_link', 'secondary', 'secondary_link', 'tertiary', 'tertiary_link'];
/** A lowered kerb's stretch is this much wider than the way that crosses it; a way at a shallow angle crosses at no less than this sine. */
const DROP_CLEAR_M = 0.5, LEAST_SINE = 0.3;
/** The surfaces that take a give-way line. */
const PAVED_SURFACES = ['main', 'road', 'oneway'];

/** Paint on a carriageway: its material, and the metres across and the count along that each vertex carries. */
export interface Paint {
    mat: number;
    across: (s: Station, x: number, y: number, a: number) => number;
    along: (s: Station, count: number) => number;
}

/** What the plan asks of a run. */
export interface RoadMarks {
    paint: (run: Run, from: number, to: number, paint: Paint) => void;
    /** Lowers the kerb on `side` (1 left, -1 right) from metres `from` to `to`. */
    lower: (run: Run, from: number, to: number, side: 1 | -1) => void;
}

const ZEBRA_ON: Record<number, number> = { [LOOK.ASPHALT]: LOOK.ZEBRA, [LOOK.MAIN]: LOOK.ZEBRA, [LOOK.MOTORWAY]: LOOK.ZEBRA, [LOOK.SETT]: LOOK.ZEBRA_SETT, [LOOK.PAVERS]: LOOK.ZEBRA_PAVERS, [LOOK.CONCRETE]: LOOK.ZEBRA_CONCRETE };

/** A point's foot on a run's stations: metres from the line and along it, and the direction across there. */
function footOn({ stations }: Run, x: number, y: number) {
    let best = { away: Infinity, along: 0, s: stations[0] };
    for (let j = 0; j + 1 < stations.length; j++) {
        const s = stations[j], t = stations[j + 1], dx = t.x - s.x, dy = t.y - s.y, u = Math.max(0, Math.min(1, ((x - s.x) * dx + (y - s.y) * dy) / (dx * dx + dy * dy || 1)));
        const away = Math.hypot(s.x + u * dx - x, s.y + u * dy - y) * metresPerUnit(y);
        if (away < best.away) best = { away, along: s.along + u * (t.along - s.along), s };
    }
    return best;
}

/**
 * A zebra's paint and lowered kerbs on the ways that hold each crossing node. Its stripes stay straight in a bend,
 * since each vertex carries its metres across the straight line through the node, and its ways share that line.
 */
export function zebras(runs: Run[], crossings: { x: number; y: number }[], { paint, lower }: RoadMarks): void {
    const paved = runs.filter(({ line }) => line.kind === ROAD && ZEBRA_ON[lookOf(line)] !== undefined);
    for (const { x, y } of crossings) {
        const feet = paved.map(run => ({ run, ...footOn(run, x, y) })), least = Math.min(...feet.map(({ away }) => away));
        if (!(least < NODE_REACH_M)) continue;
        const { s } = feet.find(({ away }) => away === least)!, l = Math.hypot(s.ax, s.ay), ux = s.ax / l, uy = s.ay / l, unit = metresPerUnit(y);
        const zebraPaint = (mat: number): Paint => ({ mat, across: (_, px, py) => ((px - x) * ux + (py - y) * uy) * unit, along: (_, count) => count });
        for (const { run, away, along, s: { ax, ay } } of feet) {
            if (away > least + SAME_FOOT_M || Math.abs(ax * ux + ay * uy) / Math.hypot(ax, ay) < ON_COS) continue;
            paint(run, along - ZEBRA_HALF_M, along + ZEBRA_HALF_M, zebraPaint(ZEBRA_ON[lookOf(run.line)]));
            lower(run, along - ZEBRA_HALF_M, along + ZEBRA_HALF_M, 1);
            lower(run, along - ZEBRA_HALF_M, along + ZEBRA_HALF_M, -1);
        }
    }
}

/** Whether a side arm gives way to its through ways: at a roundabout, or a minor paved road at a main one. A one-way arm that leaves has no traffic to stop. */
function givesWay({ line, step }: Way, through: Way[]): boolean {
    if (!PAVED_SURFACES.includes(surfaceName(line)) || (surfaceName(line) === 'oneway' && step > 0)) return false;
    if (through.some(({ line: road }) => road.roundabout)) return true;
    const main = MAIN.indexOf(classOf(through.reduce((best, way) => (way.line.profile < best.line.profile ? way : best)).line));
    return main >= 0 && !MAIN.slice(0, main + 2).includes(classOf(line));
}

/** A give-way line across a side arm's mouth that ends at metres `far`, with the triangle where the arm has room. Traffic keeps right, so the lane that stops lies on the left of a way that leaves. */
export function giveWay(way: Way, far: number, through: Way[], { paint }: RoadMarks): void {
    if (!through.length || !givesWay(way, through)) return;
    const { run: { stations }, step } = way, left = (step > 0 ? stations[stations.length - 1].along - far : far) - GIVE_WAY_BACK_M;
    const reach = left > GIVE_WAY_REACH_M + TRIANGLE_ROOM_M ? GIVE_WAY_REACH_M : GIVE_WAY_M;
    if (reach + LINE_ROOM_M >= left) return;
    const at = far + step * GIVE_WAY_BACK_M;
    paint(way.run, at, at + step * reach, { mat: LOOK.GIVE_WAY, across: (_, __, ___, a) => step * a, along: s => Math.abs(s.along - at) });
}

/** A kerb lowered where way `by` meets the through roads at their junction, on the side that it comes from. */
export function lowerAt(by: Way, roads: Way[], { lower }: RoadMarks): void {
    for (const road of roads) {
        if (!road.through) continue;
        const cross = by.u[0] * road.u[1] - by.u[1] * road.u[0], reach = (by.line.h + DROP_CLEAR_M) / Math.max(LEAST_SINE, Math.abs(cross));
        lower(road.run, road.along - reach, road.along + reach, (cross > 0 ? road.step : -road.step) as 1 | -1);
    }
}
