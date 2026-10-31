// What stands beside a road: per station and side, how far a guardrail and a retaining wall stand, and how high.
// Each station decides once, here in the worker, so every vertex of a station holds one decision. An OSM barrier
// comes first; else a rail stands where the ground drops away, a wall where it climbs. Ends ramp down as real
// terminals do. No decision reaches past a junction's mouth or a sidewalk, and no rail past a town; a wall stands in
// a town, as a town's cuts need one.
import { metresPerUnit, PROFILES, ROAD, type Barrier, type Line } from '../terrain/bins';
import { along as share, gridOf, LEFT, near, RIGHT, type Grid } from '../terrain/town';
import type { GroundAt } from './ground';
import { classOf, shoulderOf } from './section';
import { between, type Station } from './stations';

/** The classes with guardrails and delineator posts outside towns, and those with retaining walls. */
const RAILED = new Set(['trunk', 'trunk_link', 'primary', 'primary_link', 'secondary', 'secondary_link', 'tertiary', 'tertiary_link', 'unclassified']);
const WALLED = new Set([...RAILED, 'residential', 'living_street']);
/** A line's stations run on this many steps past each end: as far as a gap closes, so a wall runs on across two ways that meet. */
const TAIL_STEPS = 13, TAIL_STEP_M = 2.5;
/** Two stretches stay apart by at least this much of open ground, past their two ramps: a narrower break reads as a notch. */
const BREAK_M = 8;
/** A rail stands where the road's edge lies DROP_M over the ground DROP_OUT_M past it, and RAIL_NEAR_M along past that; then it ramps down over RAIL_RAMP_M. */
const DROP_M = 1.35, DROP_OUT_M = 4, RAIL_NEAR_M = 4, RAIL_RAMP_M = 6;
/** A wall stands where the ground WALL_OUT_M past the edge climbs CUT_M over it, and WALL_NEAR_M along past that; then it ramps down over WALL_RAMP_M. */
export const WALL_OUT_M = 2.2;
const WALL_NEAR_M = 5, WALL_RAMP_M = 7;
/** Chosen on the fixture: the stored ground, before the stamp, climbs 2.3 m and more at the reference's walls, and under 2 m at the slopes it leaves bare. */
const CUT_M = 2.3;
/** A mapped retaining wall stands where the ground WALL_OUT_M past the edge climbs at least this much over it. */
const WALL_MAPPED_M = 0.3;
/** No wall stands inside a bend so tight that its side folds this far out: there the slope belongs to both legs of a hairpin. */
const WALL_ROOM_M = 15;
/** A wall's height over the edge: the mean of the cut that its stretch passes within WALL_MEAN_M, plus WALL_OVER_M, so its top runs with the grade. */
const WALL_MEAN_M = 12, WALL_OVER_M = 0.3, WALL_LEAST_M = 0.6, WALL_MOST_M = 3.5;
/** Where a road's cut starts past its ditch, as in stamp-core.wgsl. */
const DITCH_OUT_M = 0.7;
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

/** How far a line's cut climbs `x` metres past its core, as cut_top of stamp-core.wgsl where its ditch has ended. */
export function coneOf(line: Line, x: number): number {
    const { cut, reach, ditch } = Object.values(PROFILES)[line.profile], out = x - (ditch > 0 ? DITCH_OUT_M : 0);
    return cut * out * reach / (reach - out);
}

/** The details that a line's class allows. */
export const allowed = (line: Line) => ({ railed: line.kind === ROAD && RAILED.has(classOf(line)), walled: line.kind === ROAD && WALLED.has(classOf(line)) });

/** Whether a station has a sidewalk on a side. */
const walked = (line: Line, s: Station, side: 1 | -1) => ((line.sidewalks?.[s.v] ?? 0) & (side > 0 ? LEFT : RIGHT)) !== 0;

/** One side of a line, per station. A share under 1 is a ramp: the detail dives into the ground there. */
export interface Standing {
    rail: Float64Array;
    wall: Float64Array;
    /** A standing wall's height over the road's edge. */
    high: Float64Array;
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

/** A line's stations, and past each end the straight way on: a way that meets it there runs on. */
function reached(stations: Station[]): Station[] {
    const n = stations.length, on = (s: Station, t: Station) => Array.from({ length: TAIL_STEPS }, (_, k) => between(s, t, 1 + (k + 1) * TAIL_STEP_M / (Math.abs(t.along - s.along) || 1)));
    return [...on(stations[1], stations[0]).reverse(), ...stations, ...on(stations[n - 2], stations[n - 1])];
}

/**
 * What stands on `side` of a line. `mouths` and `towns` are the metres to a line's nearest mouth, and to its nearest
 * mouth or town, per station, and `folds` how far each side reaches (reachOf of stations.ts). A station that the ground
 * does not reach passes no test. A mapped rail stands past a class's or a town's rule, and stays out of a mouth.
 */
export function standingOf(line: Line, stations: Station[], folds: Float64Array, side: 1 | -1, ground: GroundAt, mouths: Float64Array, towns: Float64Array, barriers: Barriers): Standing {
    const { railed, walled } = allowed(line), n = stations.length, all = reached(stations), m = all.length, cone = coneOf(line, WALL_OUT_M), mapped = barriers.segments.length > 0;
    const inner = (i: number) => (i >= TAIL_STEPS && i < TAIL_STEPS + n ? i - TAIL_STEPS : -1);
    const rails = new Uint8Array(m), walls = new Uint8Array(m), mappedRail = new Uint8Array(n), rise = new Float64Array(m), tags = new Float64Array(m), unknown = new Float64Array(n);
    for (let i = 0; i < m; i++) {
        const s = all[i], e = edgeOf(line, s), edge = s.z + side * s.bank * e, at = (a: number) => [s.x + side * s.ax * (e + a), s.y + side * s.ay * (e + a)] as const;
        const cut = ground(...at(WALL_OUT_M)) - edge, drop = edge - ground(...at(DROP_OUT_M)), j = inner(i);
        const found = mapped ? barrierAt(barriers, ...at(BARRIER_OUT_M)) : undefined;
        // A mapped wall on the low side of a road holds the road up, and shows nothing over it.
        const osm = found && (!found.wall || cut > WALL_MAPPED_M) ? found : undefined;
        rails[i] = osm ? +!osm.wall : +(railed && drop > DROP_M);
        walls[i] = osm ? +osm.wall : +(walled && cut > CUT_M);
        rise[i] = Math.max(0, Math.min(cone, cut));
        if (j >= 0) unknown[j] = cut;
        tags[i] = osm?.height ?? 0;
        if (j >= 0 && osm && !osm.wall) mappedRail[j] = 1;
    }
    const along = Float64Array.from(all, s => s.along), clear = zonesOf(stations, j => walked(line, stations[j], side)), free = towns.map((town, j) => Math.min(town, clear[j]));
    const stopsAt = (stop: (j: number) => boolean) => (i: number) => inner(i) >= 0 && stop(inner(i));
    // A town or a tight bend of less than BREAK_M stops nothing: a detail would dive for it and rise again.
    const lasting = (stop: (j: number) => boolean) => {
        const marks = Float64Array.from(stations, (_, j) => +stop(j));
        withoutStubs(marks, stations, BREAK_M);
        return marks;
    };
    const town = lasting(j => towns[j] <= 0), tight = lasting(j => folds[2 * j + (side > 0 ? 0 : 1)] < WALL_ROOM_M);
    const stopsRail = (j: number) => mouths[j] <= 0 || (!mappedRail[j] && (clear[j] <= 0 || town[j] > 0));
    const stopsWall = (j: number) => mouths[j] <= 0 || clear[j] <= 0 || tight[j] > 0;
    const zoneRail = zonesOf(stations, stopsRail), zoneWall = zonesOf(stations, stopsWall), samplesRail = { along, stops: stopsAt(stopsRail) }, samplesWall = { along, stops: stopsAt(stopsWall) };
    const toRail = metresTo(samplesRail, stretches(samplesRail, rails, 2 * (RAIL_NEAR_M + RAIL_RAMP_M) + BREAK_M));
    const toWall = metresTo(samplesWall, stretches(samplesWall, walls, 2 * (WALL_NEAR_M + WALL_RAMP_M) + BREAK_M));
    const rail = new Float64Array(n), wall = new Float64Array(n), high = new Float64Array(n);
    for (let j = 0; j < n; j++) {
        rail[j] = eased(toRail[j + TAIL_STEPS], RAIL_NEAR_M, RAIL_RAMP_M, zoneRail[j]);
        wall[j] = eased(toWall[j + TAIL_STEPS], WALL_NEAR_M, WALL_RAMP_M, zoneWall[j]);
    }
    withoutStubs(rail, stations, 2 * RAIL_RAMP_M);
    withoutStubs(wall, stations, 2 * WALL_RAMP_M);
    for (let j = 0; j < n; j++) {
        if (wall[j] <= 0) continue;
        let sum = 0, count = 0, tag = 0;
        for (let i = 0; i < m; i++) {
            if (!walls[i] || Math.abs(along[i] - along[j + TAIL_STEPS]) > WALL_MEAN_M) continue;
            sum += rise[i];
            count++;
            tag = Math.max(tag, tags[i]);
        }
        high[j] = Math.max(WALL_LEAST_M, Math.min(WALL_MOST_M, tag || (count ? sum / count : cone) + WALL_OVER_M));
    }
    return { rail, wall, high, free: free.map((metres, j) => (Number.isNaN(unknown[j]) ? 0 : metres)) };
}
