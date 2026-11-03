// What stands beside a road: per station and side, how far a guardrail and a retaining wall stand, and how high.
// Each station decides once, here in the worker, so every vertex of a station holds one decision. An OSM barrier
// comes first; else a rail stands where the ground drops away, and a wall where the cut has no room for its slope
// (room.ts). Ends ramp down as real terminals do. A rail stays out of a junction's mouth, off a sidewalk and out of a
// town; a wall stands at the core's edge, behind a sidewalk, and ends where a way meets its side, where a sidewalk
// starts or ends, or at a dead end.
import { metresPerUnit, PROFILES, ROAD, type Barrier, type Line } from '../terrain/bins';
import { along as share, gridOf, LEFT, near, RIGHT, type Grid } from '../terrain/town';
import type { GroundAt } from './ground';
import { coreEdgeOf, roomOf, WALL_OUT_M } from './room';
import { classOf, shoulderOf } from './section';
import { between, type Station } from './stations';
import type { Ways } from './ways';

/** The classes with guardrails and delineator posts outside towns, and those with retaining walls. */
const RAILED = new Set(['trunk', 'trunk_link', 'primary', 'primary_link', 'secondary', 'secondary_link', 'tertiary', 'tertiary_link', 'unclassified']);
const WALLED = new Set([...RAILED, 'residential', 'living_street']);
/** A line's stations run on this many steps past each end: as far as a gap closes, so a wall runs on across two ways that meet. */
const TAIL_STEPS = 13, TAIL_STEP_M = 2.5;
/** Two stretches stay apart by at least this much of open ground, past their two ramps: a narrower break reads as a notch. */
const BREAK_M = 8;
/** A rail stands where the road's edge lies DROP_M over the ground DROP_OUT_M past it, and RAIL_NEAR_M along past that; then it ramps down over RAIL_RAMP_M. */
const DROP_M = 1.35, DROP_OUT_M = 4, RAIL_NEAR_M = 4, RAIL_RAMP_M = 6;
/** A wall stands WALL_NEAR_M along past a station where it must, then ramps down over WALL_RAMP_M. */
const WALL_NEAR_M = 5, WALL_RAMP_M = 7;
/** A derived wall holds at least this much ground over the edge: a lower one is its capstone and backfill alone, a box on the ground. */
const WALL_LEAST_M = 0.6;
/** A mapped retaining wall stands where the ground WALL_OUT_M past the core's edge climbs at least this much over it. */
const WALL_MAPPED_M = 0.3;
/** A detail ramps down over ZONE_RAMP_M before what stops it. A mouth reaches past the junction's widest way by MOUTH_M. */
const ZONE_RAMP_M = 5, MOUTH_M = 5;
/** A town: TOWN_BUILDINGS buildings within TOWN_M. */
const TOWN_BUILDINGS = 3, TOWN_M = 40;
/** An OSM barrier stands at a station when it lies within BARRIER_M of a point BARRIER_OUT_M past the edge. */
const BARRIER_M = 2.5, BARRIER_OUT_M = 1;

/** The OSM barriers of a tile, per segment. */
export interface Barriers extends Grid { segments: { xy: number[]; barrier: Barrier }[] }

export function barriersOf(barriers: Barrier[]): Barriers {
    const segments = barriers.flatMap(barrier => Array.from({ length: barrier.xy.length / 2 - 1 }, (_, v) => ({ xy: barrier.xy.slice(2 * v, 2 * v + 4), barrier })));
    return { ...gridOf(segments.map(({ xy: [ax, ay, bx, by] }) => [Math.min(ax, bx), Math.min(ay, by), Math.max(ax, bx), Math.max(ay, by)])), segments };
}

/** The barrier nearest to a point within BARRIER_M, in metres. */
function barrierAt(grid: Barriers, x: number, y: number): Barrier | undefined {
    const unit = metresPerUnit(y);
    let found: Barrier | undefined, best = BARRIER_M;
    near(grid, x, y, x, y, BARRIER_M / unit, b => {
        const { xy: [ax, ay, bx, by], barrier } = grid.segments[b], t = share(ax, ay, bx, by, x, y), d = Math.hypot(ax + t * (bx - ax) - x, ay + t * (by - ay) - y) * unit;
        if (d < best) [found, best] = [barrier, d];
    });
    return found;
}

/** The share that a detail stands, `d` metres along from where it must and `zone` metres from where it may not. */
const eased = (d: number, near: number, ramp: number, zone: number) => {
    const t = Math.max(0, Math.min(1, (near + ramp - d) / ramp, zone / ZONE_RAMP_M));
    return t * t * (3 - 2 * t);
};

/** Metres along to the nearest station that `blocked` holds, in either direction; zero on one. */
export function zonesOf(stations: Station[], blocked: (j: number) => boolean): Float64Array {
    const out = new Float64Array(stations.length);
    for (let j = 0, last = -Infinity; j < stations.length; j++) {
        if (blocked(j)) last = stations[j].along;
        out[j] = stations[j].along - last;
    }
    for (let j = stations.length - 1, last = Infinity; j >= 0; j--) {
        if (blocked(j)) last = stations[j].along;
        out[j] = Math.min(out[j], last - stations[j].along);
    }
    return out;
}

/** The mouths on a line: per junction with three ways or more, its middle and the metres that it clears about it. */
export function mouthsOn(line: Line, junctions: Map<string, { x: number; y: number; h: number; ways: number }>): { x: number; y: number; gap: number }[] {
    return Array.from({ length: line.z.length }, (_, v) => junctions.get(`${line.xy[2 * v]},${line.xy[2 * v + 1]}`)).flatMap(j => (j && j.ways > 2 ? [{ x: j.x, y: j.y, gap: j.h + MOUTH_M }] : []));
}

/** Whether TOWN_BUILDINGS buildings stand within TOWN_M of a point. */
export function townish(town: Grid, x: number, y: number): boolean {
    let count = 0;
    near(town, x, y, x, y, TOWN_M / metresPerUnit(y), () => count++);
    return count >= TOWN_BUILDINGS;
}

/** The details that a line's class allows. */
export const allowed = (line: Line) => ({ railed: line.kind === ROAD && RAILED.has(classOf(line)), walled: line.kind === ROAD && WALLED.has(classOf(line)) });

/** Whether a station has a sidewalk on a side. */
const walked = (line: Line, s: Station, side: 1 | -1) => ((line.sidewalks?.[s.v] ?? 0) & (side > 0 ? LEFT : RIGHT)) !== 0;

/** One side of a line, per station. A share under 1 is a ramp: the detail dives into the ground there. */
export interface Standing {
    rail: Float64Array;
    wall: Float64Array;
    /** A standing wall's height over its core's edge: the stamped ground that it holds (room.ts). */
    high: Float64Array;
    /** The metres past the core's edge to the next way's core, Infinity where none lies: a wall's capstone ends short of it. */
    depth: Float64Array;
    /** The metres along to the nearest mouth, town or sidewalk, none where the ground is unknown: a delineator keeps away from them. */
    free: Float64Array;
}

/** The metres from a line's middle to the edge that details stand on: the outer edge of its shoulder. */
export const edgeOf = (line: Line, s: Station) => s.h + shoulderOf(line);

/** Metres along of a line's samples, and whether each is a place that stops a detail. */
interface Samples { along: Float64Array; stops: (i: number) => boolean }

/**
 * The stretches of a line where a detail must stand, from the samples that pass its test: a gap of `bridge` metres or
 * less closes, so two stretches never meet in a dip, and no stretch runs over a stop. So each stretch rises and falls
 * once, and every station of a gap holds one answer.
 */
function stretches({ along, stops }: Samples, passes: Uint8Array, bridge: number): Uint8Array {
    const out = new Uint8Array(passes.length);
    let first = -1, last = -1;
    const close = () => {
        if (first >= 0) out.fill(1, first, last + 1);
        first = -1;
    };
    for (let i = 0; i < passes.length; i++) {
        if (stops(i)) close();
        else if (passes[i]) {
            if (first >= 0 && along[i] - along[last] > bridge) close();
            if (first < 0) first = i;
            last = i;
        }
    }
    close();
    return out;
}

/** Per sample the metres to the nearest stretch that no stop cuts it off from. */
function metresTo({ along, stops }: Samples, marks: Uint8Array): Float64Array {
    const out = new Float64Array(marks.length).fill(Infinity), n = marks.length;
    for (let i = 0, from = -Infinity; i < n; i++) {
        if (stops(i)) from = -Infinity;
        else if (marks[i]) from = along[i];
        out[i] = along[i] - from;
    }
    for (let i = n - 1, from = Infinity; i >= 0; i--) {
        if (stops(i)) from = Infinity;
        else if (marks[i]) from = along[i];
        out[i] = Math.min(out[i], from - along[i]);
    }
    return out;
}

/** Takes away a stretch of `shares` shorter than `least` metres that lies inside the line: a stub between two stops. */
function withoutStubs(shares: Float64Array, stations: Station[], least: number): void {
    for (let j = 0, from = -1; j <= shares.length; j++) {
        const stands = j < shares.length && shares[j] > 0;
        if (stands && from < 0) from = j;
        if (stands || from < 0) continue;
        if (from > 0 && j < shares.length && stations[j].along - stations[from].along < least) shares.fill(0, from, j);
        from = -1;
    }
}

/** A line's stations, and past each end the straight way on: a way that meets it there runs on. Only the place runs on: the rest of the section holds at the end's. */
function reached(stations: Station[]): Station[] {
    const n = stations.length, on = (s: Station, t: Station) => Array.from({ length: TAIL_STEPS }, (_, k) => {
        const { x, y, z, along } = between(s, t, 1 + (k + 1) * TAIL_STEP_M / (Math.abs(t.along - s.along) || 1));
        return { ...t, x, y, z, along };
    });
    return [...on(stations[1], stations[0]).reverse(), ...stations, ...on(stations[n - 2], stations[n - 1])];
}

/**
 * What stands on `side` of a line. `mouths` and `towns` are the metres to a line's nearest mouth, and to its nearest
 * mouth or town, per station, `folds` how far each side reaches (reachOf of stations.ts), and `ways` the tile's ways,
 * whose cores the room test reads. A station that the ground does
 * not reach stands no rail. A mapped rail stands past a class's or a town's rule, and stays out of a mouth and off a sidewalk.
 */
export function standingOf(line: Line, stations: Station[], folds: Float64Array, side: 1 | -1, ground: GroundAt, mouths: Float64Array, towns: Float64Array, barriers: Barriers, ways: Ways): Standing {
    const { railed, walled } = allowed(line), n = stations.length, all = reached(stations), m = all.length, mapped = barriers.segments.length > 0;
    const inner = (i: number) => (i >= TAIL_STEPS && i < TAIL_STEPS + n ? i - TAIL_STEPS : -1);
    const rails = new Uint8Array(m), walls = new Uint8Array(m), mappedRail = new Uint8Array(n), held = new Float64Array(n), depth = new Float64Array(n).fill(Infinity), meets = new Uint8Array(n), unknown = new Float64Array(n);
    for (let i = 0; i < m; i++) {
        const s = all[i], e = edgeOf(line, s), w = coreEdgeOf(line, s, side), j = inner(i);
        const at = (from: number, a: number) => [s.x + side * s.ax * (from + a), s.y + side * s.ay * (from + a)] as const, edge = (from: number) => s.z + side * s.bank * from;
        const cut = ground(...at(w, WALL_OUT_M)) - edge(w), drop = edge(e) - ground(...at(e, DROP_OUT_M));
        const found = mapped ? barrierAt(barriers, ...at(e, BARRIER_OUT_M)) : undefined;
        // A mapped wall on the low side of a road holds the road up, and shows nothing over it.
        const osm = found && (!found.wall || cut > WALL_MAPPED_M) ? found : undefined;
        const room = walled || mapped ? roomOf(line, s, side, ground, ways) : undefined;
        rails[i] = osm ? +!osm.wall : +(railed && drop > DROP_M);
        walls[i] = osm ? +osm.wall : +(walled && !room!.open && room!.held >= WALL_LEAST_M);
        if (j < 0) continue;
        unknown[j] = cut;
        held[j] = room?.held ?? 0;
        depth[j] = room?.free ?? Infinity;
        meets[j] = +!!room?.meets;
        if (osm && !osm.wall) mappedRail[j] = 1;
    }
    const along = Float64Array.from(all, s => s.along), clear = zonesOf(stations, j => walked(line, stations[j], side)), free = towns.map((town, j) => Math.min(town, clear[j]));
    const stopsAt = (stop: (j: number) => boolean) => (i: number) => inner(i) >= 0 && stop(inner(i));
    // A town of less than BREAK_M stops nothing: a rail would dive for it and rise again.
    const town = Float64Array.from(stations, (_, j) => +(towns[j] <= 0));
    withoutStubs(town, stations, BREAK_M);
    // Of a mouth, a wall stops only where a way meets this side: past a junction it runs on, and between two ways that
    // leave it, it grows from their mouth. It stands at the core's edge, which steps where a sidewalk starts or ends.
    const steps = (j: number) => [j - 1, j + 1].some(k => k >= 0 && k < n && walked(line, stations[k], side) !== walked(line, stations[j], side));
    const stopsRail = (j: number) => mouths[j] <= 0 || clear[j] <= 0 || (!mappedRail[j] && town[j] > 0);
    // Inside a bend whose side folds short of the backfill's back, the section of a wall would fold.
    const folded = (j: number) => folds[2 * j + (side > 0 ? 0 : 1)] < coreEdgeOf(line, stations[j], side) + WALL_OUT_M;
    const stopsWall = (j: number) => meets[j] > 0 || steps(j) || folded(j) || (mouths[j] <= 0 && (j === 0 || j === n - 1));
    const zoneRail = zonesOf(stations, stopsRail), zoneWall = zonesOf(stations, stopsWall), samplesRail = { along, stops: stopsAt(stopsRail) }, samplesWall = { along, stops: stopsAt(stopsWall) };
    const toRail = metresTo(samplesRail, stretches(samplesRail, rails, 2 * (RAIL_NEAR_M + RAIL_RAMP_M) + BREAK_M));
    const toWall = metresTo(samplesWall, stretches(samplesWall, walls, 2 * (WALL_NEAR_M + WALL_RAMP_M) + BREAK_M));
    const rail = new Float64Array(n), wall = new Float64Array(n);
    for (let j = 0; j < n; j++) {
        rail[j] = eased(toRail[j + TAIL_STEPS], RAIL_NEAR_M, RAIL_RAMP_M, zoneRail[j]);
        wall[j] = eased(toWall[j + TAIL_STEPS], WALL_NEAR_M, WALL_RAMP_M, zoneWall[j]);
    }
    withoutStubs(rail, stations, 2 * RAIL_RAMP_M);
    withoutStubs(wall, stations, 2 * WALL_RAMP_M);
    // The top falls no steeper than the class's cut, under the held ground: where that steps, as at another way's sidewalk
    // end, the top eases down before the step.
    const { cut: grade } = Object.values(PROFILES)[line.profile], top = held.slice();
    for (let j = 1; j < n; j++) top[j] = Math.min(top[j], top[j - 1] + grade * (stations[j].along - stations[j - 1].along));
    for (let j = n - 2; j >= 0; j--) top[j] = Math.min(top[j], top[j + 1] + grade * (stations[j + 1].along - stations[j].along));
    return { rail, wall, high: top.map((metres, j) => (wall[j] > 0 ? metres : 0)), depth, free: free.map((metres, j) => (Number.isNaN(unknown[j]) ? 0 : metres)) };
}
