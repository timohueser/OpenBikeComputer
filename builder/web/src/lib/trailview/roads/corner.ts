// A corner between two neighbouring ways of a junction, as the junction plan makes it (terrain/junction-plan.ts), on
// the ways' stations: the arc that turns the kerb from the edge of the one to the edge of the other.
import type { Line } from '../terrain/bins';
import type { Corner as PlanCorner } from '../terrain/junction-plan';
import { stationAlong, type Meeting, type Way } from './arms';
import { sideOf, MAT } from './section';
import { stationOn } from './stations';

export interface Corner {
    meeting: Meeting;
    /** The two ways, in the order of their angles, and the side of each line that faces the corner. */
    p: Way;
    q: Way;
    sp: 1 | -1;
    sq: 1 | -1;
    /** The arc's centre in metres about the meeting, and its radius; 0 where the corner stays sharp. */
    o: number[];
    r: number;
    /** Metres along each line's stations at the arc's ends, and where the edges meet. */
    ends: [number, number];
    meet: [number, number];
    /** The arc's ends in metres about the meeting, how far from its centre the edges meet, and the sidewalk's width inside the arc. */
    feet: number[][];
    apex: number;
    walk: number;
    /** The through road whose surface its faces take. */
    road: Line;
    /** Its two ways are one line through its vertex, so it stays sharp and nothing is cut. */
    oneLine: boolean;
}

/** A plan's corner on the ways of a meeting: its metres along each line on that line's stations, its points about the meeting. */
export function cornerOf(corner: PlanCorner, meeting: Meeting, p: Way, q: Way, road: Line): Corner {
    const { x, y, unit } = meeting, about = ([px, py]: number[]) => [(px - x) * unit, (py - y) * unit];
    const on = (way: Way, metres: number) => stationAlong(way.run.stations, way.line, metres);
    // A side's sidewalk at the arc's end: the width past the carriageway's edge where its kerb stands.
    const walkAt = (way: Way, side: 1 | -1, metres: number) => {
        const s = stationOn(way.run.stations, on(way, metres)), { parts, edge } = sideOf(way.line, side, s.v, s.h);
        return parts.some(({ mat }) => mat === MAT.KERB) ? edge - s.h : 0;
    };
    const { sp, sq, r, apex, oneLine, ends, meet } = corner;
    return {
        meeting, p, q, sp, sq, o: about(corner.o), r, ends: [on(p, ends[0]), on(q, ends[1])], meet: [on(p, meet[0]), on(q, meet[1])], feet: [about(corner.a), about(corner.b)], apex,
        walk: Math.max(walkAt(p, sp, ends[0]), walkAt(q, sq, ends[1])), road, oneLine,
    };
}

/** Whether a point in metres about the meeting lies on a corner's fillet, or its kerb and sidewalk. */
export function onCorner({ o, r, feet: [a, b], apex, walk }: Corner, x: number, y: number): boolean {
    const cross = (p: number[], q: number[]) => (p[0] - o[0]) * (q[1] - o[1]) - (p[1] - o[1]) * (q[0] - o[0]), turn = cross(a, b), point = [x, y], d = Math.hypot(x - o[0], y - o[1]);
    return r > 0 && d > r - walk && d < apex && cross(a, point) * turn > 0 && cross(point, b) * turn > 0;
}
