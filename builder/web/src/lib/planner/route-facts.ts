/** The days, the climbs and the waypoints of a plan along its calculated line, as the Route page lists them. */
import { dayStops, planView, type ItineraryDay, type RoutePoint, type Trip } from './editor';
import { coordinateAt, type Coordinate } from './geo';
import type { RoutingLine } from './routing';
import { routeSegments } from './search/segments';

/** A waypoint of the route, numbered from 1 in route order, with its kilometres from the start. */
export interface Waypoint { point: RoutePoint; number: number; km: number }

/** A climb by the rule of the device, in kilometres from the start, with its mean grade in percent and its top. */
export interface Climb { from: number; to: number; ascent: number; gradient: number; top: Coordinate }

export interface RouteFacts {
    /** The days of the itinerary; a riding day has its waypoints, a rest day none. */
    days: (ItineraryDay & { stops: Waypoint[] })[];
    climbs: Climb[];
    stops: Waypoint[];
}

export function routeFacts(trip: Trip, line: RoutingLine): RouteFacts {
    const view = planView(trip, line);
    const stops = dayStops(trip, line, { from: 0, to: 1 }).map(({ point, km }, i) => ({ point, km, number: i + 1 }));
    const byId = new Map(stops.map(stop => [stop.point.id, stop]));
    const days = view.itinerary.map(day => ({ ...day, stops: day.rest ? [] : dayStops(trip, line, day).map(({ point }) => byId.get(point.id)!) }));
    const climbs = routeSegments(line).filter(segment => segment.kind === 'climb').map(({ from, to, ascent, gradient }) =>
        ({ from, to, ascent: ascent!, gradient: gradient!, top: coordinateAt(line.coordinates, view.total ? to / view.total : 0) }));
    return { days, climbs, stops };
}
