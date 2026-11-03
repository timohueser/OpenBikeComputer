// The ways out of the points that a tile's roads and paths share, as the junction plan finds them, with their corridors.
import { PATH, type Line } from '../terrain/bins';
import { alongOf, direction, meetingsOf as planMeetings, pointAlong, WINDOW_M, type Arm as PlanArm } from '../terrain/junction-plan';
import { stationIndex, type Station } from './stations';
import type { Run } from './sweep';

/**
 * The plan reads WINDOW_M along a way from its junction. A junction changes a way for up to REACH_M, so a tile
 * needs a junction up to REACH_M past its square, and its 40 m buffer holds that junction's ways for 16 m more.
 */
export { direction, WINDOW_M };
export const REACH_M = 24;

export interface Arm extends PlanArm {
    /** A tunnel, a gallery or a bridge without a deck's corridor that is a way of its junction draws elsewhere. */
    run?: Run;
    /** Metres along the line's stations at the junction. */
    along: number;
    /** Metres per world unit at the junction. */
    unit: number;
}

/** An arm with a corridor. */
export type Way = Arm & { run: Run };

/** A shared point, `unit` metres per world unit there. */
export interface Meeting { x: number; y: number; unit: number; roads: Arm[]; paths: Way[] }

/** The stations of a way from its junction out, the first one past `metres` included. */
export function outward({ run: { stations }, along, step }: Way, metres = WINDOW_M): Station[] {
    const out: Station[] = [];
    for (let j = stationIndex(stations, along); j >= 0 && j < stations.length; j += step) {
        out.push(stations[j]);
        if (Math.abs(stations[j].along - along) >= metres) break;
    }
    return out;
}

/** A point's foot on a way, a share u from station s to t; its metres from the foot, left of the line, and behind the junction. */
export function footOn(way: Way, x: number, y: number, list = outward(way)): { s: Station; t: Station; u: number; away: number; left: number; behind: number } {
    const { unit, step } = way;
    let best = { s: list[0], t: list[0], u: 0, away: Infinity, left: 0, behind: 0 };
    for (let k = 0; k + 1 < list.length; k++) {
        const s = list[k], t = list[k + 1], dx = (t.x - s.x) * unit, dy = (t.y - s.y) * unit, l = Math.hypot(dx, dy) || 1, ex = (x - s.x) * unit, ey = (y - s.y) * unit;
        const raw = (ex * dx + ey * dy) / l / l, u = Math.max(0, Math.min(1, raw)), d = Math.hypot(ex - u * dx, ey - u * dy);
        if (d < best.away) best = { s, t, u, away: d, left: (step * (ex * dy - ey * dx)) / l, behind: k ? 0 : Math.max(0, -raw * l) };
    }
    return best;
}

/** Metres along a line at its vertex v, from the last station before the vertex's segment: each vertex of a tile has one. */
export function vertexAlong(stations: Station[], v: number): number {
    let lo = 0, hi = stations.length - 1;
    while (lo < hi) {
        const middle = (lo + hi + 1) >> 1;
        if (stations[middle].v < v) lo = middle;
        else hi = middle - 1;
    }
    return v ? stations[lo].along : 0;
}

/** Metres along a line's stations at `metres` along its chords, as the junction plan measures a line. */
export function stationAlong(stations: Station[], line: Line, metres: number): number {
    const { v, u } = pointAlong(line, alongOf(line), metres), a = vertexAlong(stations, v);
    return a + u * (vertexAlong(stations, v + 1) - a);
}

/** The meetings of a tile's lines, as the plan has them, with each arm's corridor. */
export function meetingsOf(lines: Line[], runs: Run[]): Meeting[] {
    const runOf = new Map(runs.map(run => [run.line, run] as const));
    return planMeetings(lines).map(({ x, y, unit, arms }) => {
        const out = arms.map((arm): Arm => {
            const run = runOf.get(arm.line);
            return { ...arm, run, along: run ? vertexAlong(run.stations, arm.v) : 0, unit };
        });
        return { x, y, unit, roads: out.filter(({ line }) => line.kind !== PATH), paths: out.filter((arm): arm is Way => arm.line.kind === PATH && !!arm.run) };
    });
}
