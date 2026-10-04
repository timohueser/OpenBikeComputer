import { loopTrip, type Coordinate, type RoutePoint, type Trip } from './editor';
import { decodeCoordinates } from './route-answer';
import { routePlan, type RouteRecord } from './signed-routes';

/** The route API takes at most this many points. */
const MAX_POINTS = 64;

export interface RoutePlan { points: Coordinate[]; turnarounds: number[] }

/**
 * The plan of a route record. A loop that starts at line vertex `start` runs from there around the loop, through the
 * data start, back to `start`. A rotation that needs more points than a request takes keeps the data start.
 */
export function recordPlan(route: RouteRecord, start = 0): RoutePlan {
    if (!route.loop || start <= 0) return routePlan(route);
    const line = decodeCoordinates(route.line_udeg);
    const ends = [0, ...route.via];
    const order = [start, ...ends.filter(i => i > start), ...ends.filter(i => i < start), start];
    if (order.length > MAX_POINTS) return routePlan(route);
    const turns = new Set(route.turnarounds ?? []);
    return {
        points: order.map(i => line[i]),
        turnarounds: order.flatMap((i, k) => k > 0 && k < order.length - 1 && turns.has(i) ? [k] : []),
    };
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

/** A plan of `base` that follows `plan`. Its shaping points draw no pin; at a turnaround the route turns back. A loop plan ends at its start. */
export function planTrip(base: Trip, { points, turnarounds }: RoutePlan, loop: boolean): Trip {
    const shape = (point: RoutePoint, index: number): RoutePoint => point.kind !== 'via' ? point
        : { ...point, hidden: true, ...turnarounds.includes(index) ? { turnaround: true } : {} };
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
