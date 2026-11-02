// The ways out of the points that a tile's roads and paths share. The bake marks which of them leave as a side.
import { SIDE_BACK, SIDE_ON } from '../stream/features';
import { BRIDGE, isPath, metresPerUnit, PATH, RAIL_CODE, ROAD, TUNNEL, type Line } from '../terrain/bins';
import { isDeck } from './decks';
import { MAT, sideOf } from './section';
import { stationIndex, stationOn, type Station } from './stations';
import type { Run } from './sweep';

/**
 * The plan reads this many metres along a way from its junction. A junction changes a way for up to REACH_M, so a tile
 * needs a junction up to REACH_M past its square, and its 40 m buffer holds that junction's ways for 16 m more.
 */
export const WINDOW_M = 16, REACH_M = 24;
/** A way's direction is toward its point this far on, as in the bake. */
const LOOK_M = 8;
/** The most metres of a way that a corner takes, and its share of the way to the way's end near by. */
export const ROOM_M = 14;
const ROOM_SHARE = 0.45;

export interface Arm {
    line: Line;
    /** A tunnel, a gallery or a bridge without a deck's corridor that is a way of its junction draws elsewhere. */
    run?: Run;
    /** The vertex of the junction and the step away from it. */
    v: number;
    step: 1 | -1;
    /** Metres along the line at the junction. */
    along: number;
    /** The unit direction in metres east and south, and its angle. */
    u: [number, number];
    angle: number;
    through: boolean;
    /** The metres along it that a corner may take. */
    room: number;
    /** Metres per world unit at the junction. */
    unit: number;
    /** On a side arm with a mouth: metres along the line where its whole carriageway starts. */
    mouth?: number;
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

/** Whether a way has a sidewalk near its junction. A line's far end says nothing: it depends on the tile. */
export const inTown = (way: Way) => outward(way).some(({ v }) => way.line.sidewalks?.[v]);

/** The station of a way at its junction. */
export const junctionStation = ({ run: { stations }, along }: Way) => stationOn(stations, along);

/** A side of a way at its junction: its core's outer edge in metres from the middle, the sidewalk's metres past the carriageway, 0 without a kerb, and whether a verge of the ground lies there. */
export function sideAt(way: Way, side: 1 | -1): { edge: number; walk: number; verge: boolean } {
    const s = junctionStation(way), { parts, edge } = sideOf(way.line, side, s.v, s.h);
    return { edge, walk: parts.some(({ mat }) => mat === MAT.KERB) ? edge - s.h : 0, verge: parts.some(({ mat, curtain }) => mat === MAT.GROUND && !curtain) };
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

/** A way out of vertex v of a line, toward its point LOOK_M on. */
function direction(line: Line, v: number, step: 1 | -1, unit: number): [number, number] {
    const x = line.xy[2 * v], y = line.xy[2 * v + 1];
    let left = LOOK_M, px = x, py = y;
    for (let k = v + step; k >= 0 && k < line.z.length && left > 0; k += step) {
        const qx = line.xy[2 * k], qy = line.xy[2 * k + 1], piece = Math.hypot(qx - px, qy - py) * unit, u = Math.min(1, left / (piece || 1));
        [px, py, left] = [px + u * (qx - px), py + u * (qy - py), left - piece];
    }
    const dx = (px - x) * unit, dy = (py - y) * unit, l = Math.hypot(dx, dy) || 1;
    return [dx / l, dy / l];
}

/** The metres of a way that a corner may take: short of the way's end, or of the next junction, if either lies in the window. */
function roomOf({ line, v, step }: Arm, shared: Set<string>, unit: number): number {
    const last = line.z.length - 1;
    let d = 0;
    for (let k = v + step; k >= 0 && k <= last && d < WINDOW_M; k += step) {
        d += Math.hypot(line.xy[2 * k] - line.xy[2 * k - 2 * step], line.xy[2 * k + 1] - line.xy[2 * k + 1 - 2 * step]) * unit;
        if (k === 0 || k === last || shared.has(`${line.xy[2 * k]},${line.xy[2 * k + 1]}`)) return d < WINDOW_M ? Math.min(ROOM_M, ROOM_SHARE * d) : ROOM_M;
    }
    return ROOM_M;
}

/** Metres along a line at its vertex v, from the last station before the vertex's segment: each vertex of a tile has one. */
function vertexAlong(stations: Station[], v: number): number {
    let lo = 0, hi = stations.length - 1;
    while (lo < hi) {
        const middle = (lo + hi + 1) >> 1;
        if (stations[middle].v < v) lo = middle;
        else hi = middle - 1;
    }
    return v ? stations[lo].along : 0;
}

/** The meetings of a tile's lines: points of two ways or more, with their arms sorted by angle. */
export function meetingsOf(lines: Line[], runs: Run[]): Meeting[] {
    const runOf = new Map(runs.map(run => [run.line, run] as const)), at = new Map<string, { line: Line; v: number }[]>();
    const counts = (line: Line) => runOf.has(line) ? line.kind === ROAD || line.kind === PATH || isDeck(line) : (line.kind === BRIDGE || line.kind === TUNNEL || !!line.gallery) && !isPath(line) && line.code < RAIL_CODE;
    for (const line of lines) {
        if (!counts(line)) continue;
        for (let v = 0; v < line.z.length; v++) {
            // A deck is a way of a junction at its ends only: what crosses its inner vertices passes under it.
            if (!(line.anchors?.[v] ?? true) || (isDeck(line) && v > 0 && v + 1 < line.z.length)) continue;
            const name = `${line.xy[2 * v]},${line.xy[2 * v + 1]}`;
            (at.get(name) ?? at.set(name, []).get(name)!).push({ line, v });
        }
    }
    const shared = new Set<string>(), out: Meeting[] = [];
    for (const [name, here] of at) if (here.length > 1) shared.add(name);
    for (const here of [...shared].map(name => at.get(name)!)) {
        const { line: first, v: at0 } = here[0], x = first.xy[2 * at0], y = first.xy[2 * at0 + 1], unit = metresPerUnit(y);
        const arms = here.flatMap(({ line, v }) => ([[-1, SIDE_BACK], [1, SIDE_ON]] as const).filter(([step]) => v + step >= 0 && v + step < line.z.length).map(([step, bit]) => {
            const run = runOf.get(line), u = direction(line, v, step, unit);
            const arm: Arm = { line, run, v, step, along: run ? vertexAlong(run.stations, v) : 0, u, angle: Math.atan2(u[1], u[0]), through: !((line.sideWays?.[v] ?? 0) & bit), room: 0, unit };
            arm.room = roomOf(arm, shared, unit);
            return arm;
        }));
        arms.sort((p, q) => p.angle - q.angle);
        out.push({ x, y, unit, roads: arms.filter(({ line }) => line.kind !== PATH), paths: arms.filter((arm): arm is Way => arm.line.kind === PATH && !!arm.run) });
    }
    return out;
}
