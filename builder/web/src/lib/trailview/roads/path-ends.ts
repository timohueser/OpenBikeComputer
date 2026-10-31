// A path or a driveway that meets roads at a shared point stops at their outer edge: it does not lie over the
// carriageway, and its tread meets the road's sidewalk or shoulder level.
import { footOn, outward, WINDOW_M, type Way } from './arms';
import { onCorner, type Corner } from './corner';
import type { Mark } from './junctions';
import { MAT, sideOf } from './section';
import { between, CUT, stationAt, type Station } from './stations';

/** A way's end: metres along its line, the road that it meets, and whether a sidewalk lies there. */
export interface PathEnd { way: Way; at: number; road: Way; walk: boolean }

/** The search's step along a path; a road's core reaches this far past its half-width behind its junction. */
const STEP_M = 0.1, BEHIND_M = 0.5;
/** A path keeps at least this much of its line past its end. */
const LEAST_M = 0.2;

/** The station `metres` out along a way from its junction, made up. */
function stationOut(way: Way, list: Station[], metres: number): Station {
    const k = list.findIndex(s => Math.abs(s.along - way.along) >= metres);
    if (k < 1) return list[k < 0 ? list.length - 1 : 0];
    const [s, t] = [list[k - 1], list[k]];
    return between(s, t, (way.along + way.step * metres - s.along) / (t.along - s.along));
}

/** The core's outer edge on a road's side, and whether a sidewalk lies on it. */
function coreOf({ run: { stations }, along, line }: Way, side: 1 | -1): { edge: number; walk: boolean } {
    const s = stations[stationAt(stations, along)], { parts, edge } = sideOf(line, side, s.v, s.h);
    return { edge, walk: parts.some(({ mat }) => mat === MAT.SIDEWALK) };
}

/** Where each of a junction's paths leaves its roads' cores and its corners: they stop there. */
export function endsOf(roads: Way[], paths: Way[], corners: Corner[], mark: Mark): PathEnd[] {
    const cores = roads.map(road => ({ road, list: outward(road), sides: [coreOf(road, 1), coreOf(road, -1)] })), ends: PathEnd[] = [];
    /** The core or the corner that holds a world point. */
    const heldAt = (x: number, y: number) => {
        for (const { road, list, sides } of cores) {
            const { left, behind, s } = footOn(road, x, y, list), core = sides[left > 0 ? 0 : 1];
            if (behind <= s.h + BEHIND_M && Math.abs(left) < core.edge) return { road, walk: core.walk };
        }
        const corner = corners.find(one => onCorner(one, (x - one.meeting.x) * one.meeting.unit, (y - one.meeting.y) * one.meeting.unit));
        return corner && { road: corner.p, walk: corner.walk > 0 };
    };
    for (const way of paths) {
        const total = way.run.stations[way.run.stations.length - 1].along, list = outward(way);
        let metres = 0, held: ReturnType<typeof heldAt>;
        for (; metres < WINDOW_M; metres += STEP_M) {
            const s = stationOut(way, list, metres), hit = [-1, 0, 1].map(a => heldAt(s.x + s.ax * s.h * a, s.y + s.ay * s.h * a)).find(Boolean);
            if (!hit) break;
            held = hit;
        }
        const at = way.along + way.step * metres;
        if (!held || metres >= WINDOW_M || at <= LEAST_M || at >= total - LEAST_M) continue;
        mark(way, way.along, at, CUT.ALL);
        ends.push({ way, at, ...held });
    }
    return ends;
}
