import { emptyTrip, loopTrip, type Coordinate, type RoutePoint, type Trip } from './editor';
import type { BikeType } from './riding-profiles';
import { decodeCoordinates } from './route-answer';
import { nearestVertex, routePlan, type CatalogRecord, type RouteRecord } from './signed-routes';

/** The route API takes at most this many points. */
const MAX_POINTS = 64;

/** Index 0 in `turnarounds` marks a loop start where the route turns back; it is a turnaround once the start moves. */
export interface RoutePlan { points: Coordinate[]; turnarounds: number[] }

/** The line vertex where a loop starts nearest to `place`, or 0 (its own start) when that start needs more points than a request takes. */
export function loopStart(route: RouteRecord, place: Coordinate): number {
    const vertex = nearestVertex(decodeCoordinates(route.line_udeg), place);
    return route.via.length + (route.via.includes(vertex) ? 2 : 3) <= MAX_POINTS ? vertex : 0;
}

/** The plan of a route record. A loop that starts at line vertex `start` runs from there around the loop, through its own start, back to `start`. */
export function recordPlan(route: RouteRecord, start = 0): RoutePlan {
    if (!route.loop) return routePlan(route);
    const line = decodeCoordinates(route.line_udeg);
    const ends = [0, ...route.via];
    const order = start > 0 ? [start, ...ends.filter(i => i > start), ...ends.filter(i => i < start), start] : [...ends, line.length - 1];
    const turns = new Set(route.turnarounds ?? []);
    return { points: order.map(i => line[i]), turnarounds: order.flatMap((i, k) => k < order.length - 1 && turns.has(i) ? [k] : []) };
}

/** The stage plans of a long route in one plan, each stage finish joined to the next stage start. Null when it needs more points than a request takes. */
export function joinedPlan(stages: RoutePlan[]): RoutePlan | null {
    const joined: RoutePlan = { points: [], turnarounds: [] };
    for (const { points, turnarounds } of stages) {
        const last = joined.points.at(-1);
        const skip = last && last[0] === points[0][0] && last[1] === points[0][1] ? 1 : 0;
        const offset = joined.points.length - skip;
        joined.turnarounds.push(...turnarounds.map(index => index + offset));
        joined.points.push(...points.slice(skip));
    }
    return joined.points.length <= MAX_POINTS ? joined : null;
}

/**
 * The empty plan that a signed route fills: named after the route, with the Balanced preset of an activity that rides
 * its kind, which the shaping points of the catalog reproduce.
 */
export function routeBase(route: CatalogRecord, trip: Trip): Trip {
    const bike = trip.bike ?? 'touring';
    const activity: BikeType = route.kind === 'hiking' || route.kind === 'foot' ? 'hiking' : route.kind === 'mtb' ? 'mtb'
        : ['road', 'gravel', 'touring'].includes(bike) ? bike : 'touring';
    return { ...emptyTrip(trip.mode), name: route.name ?? route.ref, bike: activity, preset: 'Balanced', startDate: trip.startDate };
}

/** A plan of `base` that follows `plan`. Its shaping points draw no pin; at a turnaround the route turns back. A loop plan ends at its start. */
export function planTrip(base: Trip, { points, turnarounds }: RoutePlan, loop: boolean): Trip {
    const shape = (point: RoutePoint, index: number): RoutePoint => ({
        ...point, ...point.kind === 'via' ? { hidden: true as const } : {}, ...turnarounds.includes(index) ? { turnaround: true as const } : {},
    });
    if (loop) {
        const trip = loopTrip(base, points.slice(0, -1));
        return { ...trip, points: trip.points.map(shape) };
    }
    const last = points.length - 1;
    const route = points.map((coordinate, i): RoutePoint => shape({
        id: crypto.randomUUID(), coordinate: [...coordinate], progress: i / last,
        ...i === 0 ? { kind: 'start', label: 'Start' } : i === last ? { kind: 'finish', label: 'Finish' } : { kind: 'via', label: 'Shaping point' },
    }, i));
    return { ...base, loop: undefined, routing: undefined, splits: undefined, points: route, routeOrder: route.slice(1, -1).map(point => point.id) };
}
