import { emptyTrip, loopTrip, type RoutePoint, type Trip } from './editor';
import type { Coordinate } from './geo';
import { presetName, type BikeType } from './riding-profiles';
import { catalogProfile, type CatalogRecord } from './signed-routes';

/** The route API takes at most this many points. */
const MAX_POINTS = 64;

/** Index 0 in `turnarounds` marks a loop start where the route turns back. The route request sends only interior indices, so it counts once "Start the loop here" moves the start. */
export interface RoutePlan { points: Coordinate[]; turnarounds: number[] }

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

/** The empty plan that reproduces the catalog route with its required activity and terrain capability. */
export function routeBase(route: CatalogRecord, trip: Trip): Trip {
    const profile = catalogProfile(route, trip.bike);
    if (!profile) throw new Error('The catalog route has an invalid routing profile.');
    return { ...emptyTrip(trip.mode), name: route.name ?? route.ref, bike: profile.split('/')[0] as BikeType, preset: presetName(profile), startDate: trip.startDate };
}

/** A plan of `base` that follows `plan`, from the route's own start. At a turnaround the route turns back. A loop plan ends at its start. */
export function planTrip(base: Trip, { points, turnarounds }: RoutePlan, loop: boolean): Trip {
    const shape = (point: RoutePoint, index: number): RoutePoint => turnarounds.includes(index) ? { ...point, turnaround: true } : point;
    if (loop) {
        const trip = loopTrip(base, points.slice(0, -1));
        return { ...trip, points: trip.points.map(shape) };
    }
    const last = points.length - 1;
    const route = points.map((coordinate, i): RoutePoint => shape({
        id: crypto.randomUUID(), coordinate: [...coordinate],
        ...i === 0 ? { kind: 'start', label: 'Start' } : i === last ? { kind: 'finish', label: 'Finish' } : { kind: 'via', label: 'Shaping point' },
    }, i));
    return { ...base, loop: undefined, splits: undefined, points: route, routeOrder: route.slice(1, -1).map(point => point.id) };
}
