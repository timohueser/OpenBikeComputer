// What stands beside a road: per station and side, how far a guardrail and a retaining wall stand, and how high.
// Each station decides once, here in the worker, so every vertex of a station holds one decision. An OSM barrier
// comes first; else a rail stands where the ground drops away, a wall where it climbs. Ends ramp down as real
// terminals do, and no decision reaches past a junction's mouth, a town or a sidewalk.
import { metresPerUnit, PROFILES, ROAD, type Barrier, type Line } from '../terrain/bins';
import { along as share, gridOf, LEFT, near, RIGHT, type Grid } from '../terrain/town';
import type { GroundAt } from './ground';
import { classOf, shoulderOf } from './section';
import { stationAt, type Station } from './stations';

/** The classes with guardrails and delineator posts outside towns, and those with retaining walls. */
const RAILED = new Set(['trunk', 'trunk_link', 'primary', 'primary_link', 'secondary', 'secondary_link', 'tertiary', 'tertiary_link', 'unclassified']);
const WALLED = new Set([...RAILED, 'residential', 'living_street']);
/** A station samples the ground SAMPLES steps along each way. */
const SAMPLES = 4;
/** A rail stands where the road's edge lies DROP_M over the ground DROP_OUT_M past it, and RAIL_NEAR_M along past that; then it ramps down over RAIL_RAMP_M. */
const DROP_M = 1.35, DROP_OUT_M = 4, RAIL_STEP_M = 2.5, RAIL_NEAR_M = 4, RAIL_RAMP_M = 6;
/**
 * A wall stands where the ground WALL_OUT_M past the edge climbs CUT_M over it, and WALL_NEAR_M along past that; then
 * it ramps down over WALL_RAMP_M. The stored ground reads before the stamp: its cut stops at the cone (coneOf), so
 * CUT_M lies over the cone's top there, and a slope that a cone barely shapes keeps no wall.
 */
export const WALL_OUT_M = 2.2;
const CUT_M = 2.3, WALL_STEP_M = 3, WALL_NEAR_M = 5, WALL_RAMP_M = 7;
/** No wall stands inside a bend so tight that its side folds this far out: there the slope belongs to both legs of a hairpin. */
const WALL_ROOM_M = 15;
/** A wall's height over the edge: the cut's mean over its samples plus WALL_OVER_M, so its top runs with the grade. */
const WALL_OVER_M = 0.3, WALL_LEAST_M = 0.6, WALL_MOST_M = 3.5;
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
    /** The metres along to the nearest mouth, town or sidewalk: a delineator keeps away from them. */
    free: Float64Array;
}

/** The metres from a line's middle to the edge that details stand on: the outer edge of its shoulder. */
export const edgeOf = (line: Line, s: Station) => s.h + shoulderOf(line);

/** A test of the ground `out` metres past the edge: dir 1 for how far it lies under the edge's grade line, -1 for how far over. */
interface Test { out: number; dir: 1 | -1; level: number; step: number; wall: boolean }
const RAIL_TEST: Test = { out: DROP_OUT_M, dir: 1, level: DROP_M, step: RAIL_STEP_M, wall: false };
const WALL_TEST: Test = { out: WALL_OUT_M, dir: -1, level: CUT_M, step: WALL_STEP_M, wall: true };

/**
 * By samples along a side of a line, from a point `at` metres along: the metres to the nearest sample that a test
 * passes, short of a zone `zone` metres away; the weighted mean of how far the ground lies past the edge, up to
 * `limit`; and a mapped height. A crossing between two samples is interpolated, and a mapped barrier passes the test
 * of its kind and stops the other. A point with a passing sample each way lies in a gap, which a detail bridges: a
 * dip in a wall's top would read as two walls.
 */
function tested(line: Line, stations: Station[], j: number, side: 1 | -1, test: Test, limit: number, zone: number, ground: GroundAt, barriers: Barriers) {
    const mapped = barriers.segments.length > 0;
    let d = Infinity, last = -1, sum = 0, weights = 0, tagged = 0, before = false, after = false;
    for (let k = -SAMPLES; k <= SAMPLES; k++) {
        const x = k * test.step, s = stationAt(stations, j, x), edge = edgeOf(line, s), out = (a: number) => [s.x + side * s.ax * (edge + a), s.y + side * s.ay * (edge + a)] as const;
        const g = ground(...out(test.out)), r = test.dir * (s.z + side * s.bank * edge - g), osm = mapped ? barrierAt(barriers, ...out(BARRIER_OUT_M)) : undefined;
        let v = -1;
        if (!Number.isNaN(g) && Math.abs(x) < zone) {
            v = osm ? (osm.wall === test.wall ? 1 : -1) : r - test.level;
            const weight = SAMPLES + 1 - Math.abs(k);
            sum += weight * Math.max(0, Math.min(limit, r));
            weights += weight;
            if (osm?.wall === test.wall) tagged = Math.max(tagged, osm.height ?? 0);
        }
        if (v > 0) {
            d = Math.min(d, Math.abs(x));
            before ||= k < 0;
            after ||= k > 0;
        }
        if (k > -SAMPLES && v > 0 !== last > 0) d = Math.min(d, Math.abs(x - test.step * v / (v - last)));
        last = v;
    }
    return { d: before && after ? 0 : d, mean: sum / (weights || 1), tagged };
}

/**
 * What stands on `side` of a line. `mouths` and `towns` are the metres to a line's nearest mouth, and to its nearest
 * mouth or town, per station, and `folds` how far each side reaches (reachOf of stations.ts). A station that `wanted`
 * leaves out, or that the ground does not reach, holds nothing.
 */
export function standingOf(line: Line, stations: Station[], folds: Float64Array, side: 1 | -1, ground: GroundAt, mouths: Float64Array, towns: Float64Array, barriers: Barriers, wanted: (j: number) => boolean = () => true): Standing {
    const { railed, walled } = allowed(line), n = stations.length, cone = coneOf(line, WALL_OUT_M);
    const clear = zonesOf(stations, j => walked(line, stations[j], side)), free = towns.map((town, j) => Math.min(town, clear[j]));
    const walls = zonesOf(stations, j => folds[2 * j + (side > 0 ? 0 : 1)] < WALL_ROOM_M);
    const rail = new Float64Array(n), wall = new Float64Array(n), high = new Float64Array(n);
    for (let j = 0; j < n; j++) {
        if (!wanted(j)) continue;
        if (railed && free[j] > 0) rail[j] = eased(tested(line, stations, j, side, RAIL_TEST, 0, free[j], ground, barriers).d, RAIL_NEAR_M, RAIL_RAMP_M, free[j]);
        const zone = Math.min(mouths[j], clear[j], walls[j]);
        if (!walled || zone <= 0) continue;
        const { d, mean, tagged } = tested(line, stations, j, side, WALL_TEST, cone, zone, ground, barriers);
        wall[j] = eased(d, WALL_NEAR_M, WALL_RAMP_M, zone);
        high[j] = Math.max(WALL_LEAST_M, Math.min(WALL_MOST_M, tagged || mean + WALL_OVER_M));
    }
    return { rail, wall, high, free };
}
