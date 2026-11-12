/** The days, the climbs and the waypoints of a plan along its calculated line, as the Route page lists them. */
import { dayStops, planView, stopKinds, type ItineraryDay, type RoutePoint, type Trip } from './editor';
import { coordinateAt, cumulative, nearestProgress, type Coordinate } from './geo';
import { profileHeightAt, profileSamples } from './profile-data';
import type { RoutingLine } from './routing';
import { routeSegments } from './search/segments';

/** A waypoint of the route, numbered from 1 in route order, with its kilometres from the start. */
export interface Waypoint { point: RoutePoint; number: number; km: number }

/** A climb by the rule of the device, in kilometres from the start, with its mean grade in percent, the mean grade of its
 * steepest STEEPEST_KM, and its top. */
export interface Climb { from: number; to: number; ascent: number; gradient: number; steepest: number; top: Coordinate }

/** The length of the stretch that `Climb.steepest` measures. Chosen: a shorter stretch shows the errors of single heights,
 * such as a bridge in the terrain model. */
export const STEEPEST_KM = .5;

export interface RouteFacts {
    /** The days of the itinerary; a riding day has its waypoints, a rest day none. */
    days: (ItineraryDay & { stops: Waypoint[] })[];
    climbs: Climb[];
    stops: Waypoint[];
}

/** A waypoint (with its number) or a night (without) at a share of the line from 0 to 1. */
export interface Pin { share: number; label: string; number?: number; point?: RoutePoint }

export function routeFacts(trip: Trip, line: RoutingLine): RouteFacts {
    const view = planView(trip, line);
    const stops = dayStops(trip, line, { from: 0, to: 1 }).map(({ point, km }, i) => ({ point, km, number: i + 1 }));
    const byId = new Map(stops.map(stop => [stop.point.id, stop]));
    const days = view.itinerary.map(day => ({ ...day, stops: day.rest ? [] : dayStops(trip, line, day).map(({ point }) => byId.get(point.id)!) }));
    const km = cumulative(line.coordinates), samples = profileSamples(line), total = km.at(-1) || 1;
    // The heights are linear between the points, so the steepest window starts or ends at a point.
    const steepest = (from: number, to: number, gradient: number) => {
        let most = -Infinity;
        for (const start of km.flatMap(at => [at, at - STEEPEST_KM])) {
            if (start < from || start + STEEPEST_KM > to) continue;
            const low = profileHeightAt(start / total, samples), high = profileHeightAt((start + STEEPEST_KM) / total, samples);
            if (low !== null && high !== null) most = Math.max(most, (high - low) / STEEPEST_KM / 10);
        }
        return Number.isFinite(most) ? most : gradient;
    };
    const climbs = routeSegments(line).filter(segment => segment.kind === 'climb').map(({ from, to, ascent, gradient }) =>
        ({ from, to, ascent: ascent!, gradient: gradient!, steepest: steepest(from, to, gradient!), top: coordinateAt(line.coordinates, view.total ? to / view.total : 0) }));
    return { days, climbs, stops };
}

/** The waypoints and the nights of a plan: on the calculated line when there is one, else at the points of `coordinates`
 * nearest to them, so that they show with no network. */
export function routePins(trip: Trip, coordinates: Coordinate[], line?: RoutingLine): Pin[] {
    if (line) {
        const view = planView(trip, line), total = view.total || 1;
        return [
            ...dayStops(trip, line, { from: 0, to: 1 }).map(({ point, km }, i) => ({ share: km / total, label: point.label, number: i + 1, point })),
            ...view.days.slice(0, -1).map(day => ({ share: day.to, label: day.pinned?.label ?? '' })),
        ];
    }
    const at = (point: RoutePoint) => nearestProgress(coordinates, point.coordinate);
    const stops = trip.points.filter(point => stopKinds.includes(point.kind)).map(point => ({ share: at(point), label: point.label, point }));
    return [
        ...stops.sort((a, b) => a.share - b.share).map((pin, i) => ({ ...pin, number: i + 1 })),
        ...trip.points.filter(point => point.kind === 'night').map(point => ({ share: at(point), label: point.label })),
    ];
}
