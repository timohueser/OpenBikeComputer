// A path or a driveway that meets roads at a shared point stops at their outer edge: it does not lie over the
// carriageway, and its tread meets the road's sidewalk or shoulder level.
import { footOn, outward, sideAt, WINDOW_M, type Way } from './arms';
import { onCorner, type Corner } from './corner';
import type { Mark } from './junctions';
import { frays } from './section';
import { between, CUT, type Station } from './stations';

/** A way's end: metres along its line, the roads that it meets, and whether a sidewalk lies there. */
export interface PathEnd { way: Way; at: number; roads: Way[]; walk: boolean }

/** The search's step along a path; a road's core reaches this far past its half-width behind its junction. */
const STEP_M = 0.1, BEHIND_M = 0.5;
/** A way of soil holds a path at this share of its half-width, by the path's middle: its margin is the ground, and a tread that stops at its edge stops short of its ruts. */
const SOIL_SHARE = 0.8;
/** A path keeps at least this much of its line past its end. */
const LEAST_M = 0.2;

/** The station `metres` out along a way from its junction, made up. */
function stationOut(way: Way, list: Station[], metres: number): Station {
    const k = list.findIndex(s => Math.abs(s.along - way.along) >= metres);
    if (k < 1) return list[k < 0 ? list.length - 1 : 0];
    const [s, t] = [list[k - 1], list[k]];
    return between(s, t, (way.along + way.step * metres - s.along) / (t.along - s.along));
}

/** Where each of a junction's paths leaves its roads' cores and its corners: they stop there. */
export function endsOf(roads: Way[], paths: Way[], corners: Corner[], mark: Mark): PathEnd[] {
    const cores = roads.map(road => ({ road, list: outward(road), sides: [sideAt(road, 1), sideAt(road, -1)] })), ends: PathEnd[] = [];
    /** The core or the corner that holds a world point. */
    const heldAt = (x: number, y: number, paved: boolean, middle: boolean) => {
        for (const { road, list, sides } of cores) {
            const { left, behind, s } = footOn(road, x, y, list), core = sides[left > 0 ? 0 : 1];
            // A road that ends here ends plainly: nothing of it lies past its last station.
            const soil = frays(road.line);
            // A paved way runs on over a verge, which is ground, to the carriageway.
            const edge = soil ? SOIL_SHARE * s.h : paved && core.verge ? s.h : core.edge;
            if ((middle || !soil) && behind <= (roads.length > 1 ? s.h + BEHIND_M : 0) && Math.abs(left) < edge) return { walk: core.walk > 0 };
        }
        const corner = corners.find(one => onCorner(one, (x - one.meeting.x) * one.meeting.unit, (y - one.meeting.y) * one.meeting.unit));
        return corner && { walk: corner.walk > 0 };
    };
    for (const way of paths) {
        const total = way.run.stations[way.run.stations.length - 1].along, list = outward(way);
        let metres = 0, held: ReturnType<typeof heldAt>;
        for (; metres < WINDOW_M; metres += STEP_M) {
            const s = stationOut(way, list, metres), hit = [-1, 0, 1].map(a => heldAt(s.x + s.ax * s.h * a, s.y + s.ay * s.h * a, !frays(way.line), a === 0)).find(Boolean);
            if (!hit) break;
            held = hit;
        }
        const at = way.along + way.step * metres, end = way.step > 0 ? total : 0;
        // A short way that lies on the road to its last point, as a mapped crossing does, draws none of it.
        if (held && Math.abs(end - way.along) <= metres + LEAST_M) mark(way, way.along, end, CUT.ALL);
        if (!held || metres >= WINDOW_M || at <= LEAST_M || at >= total - LEAST_M) continue;
        mark(way, way.along, at, CUT.ALL);
        ends.push({ way, at, roads, walk: held.walk });
    }
    return ends;
}
