import { coordinateAt, cumulative, kilometres, routeSlice, type Coordinate } from './geo';
import type { RoutePoint } from './editor';
import { noteDistance, type Stretch } from './route-closures';

/** A shorter gap is a coarse click, or a building set back from its road: a map pixel at zoom 10 is about 100 m. */
const shortestKm = 0.1;

export interface RouteGap {
    point: RoutePoint;
    /** From the route to the point, where the rider put it. */
    coordinates: [Coordinate, Coordinate];
    km: number;
    /** The km along the route of the point's stop. */
    at: number;
}

/** The points that the route does not reach, because no accessible road is nearer. The router attaches such a point to
 * the nearest road within 1 km (`specs/route-api.md`). */
export function routeGaps(stops: { point: RoutePoint; distance: number }[], coordinates: Coordinate[]): RouteGap[] {
    const total = cumulative(coordinates).at(-1) ?? 0;
    if (!total) return [];
    // A loop lists its start again as the closing stop.
    const seen = new Set<string>();
    return stops.flatMap(({ point, distance }, i) => {
        // A manual leg from the point already joins the route to it; after a detour, it starts at the anchor.
        if ((point.kind !== 'detour' && (stops[i + 1]?.point.leg ?? 'routed') !== 'routed') || seen.has(point.id)) return [];
        const attached = coordinateAt(coordinates, distance / total);
        const km = kilometres(attached, point.coordinate);
        if (km <= shortestKm) return [];
        seen.add(point.id);
        return [{ point, coordinates: [attached, point.coordinate], km, at: distance }];
    });
}

/** Such as "Ends 180 m before your point". */
export function gapLabel(gap: RouteGap): string {
    const distance = noteDistance(gap.km, 10);
    const kind = gap.point.kind;
    return kind === 'start' ? `Starts ${distance} from your point`
        : kind === 'finish' ? `Ends ${distance} before your point`
        : `Passes ${distance} from your point`;
}

/** The legs with no legal path, by the ids of the points at which they end (`RoutingLine.gapLegs`): each from the stop
 * before it to its own stop. */
export function legGaps(stops: { point: RoutePoint; distance: number }[], coordinates: Coordinate[], ids: string[]): Stretch[] {
    const total = cumulative(coordinates).at(-1) ?? 0;
    if (!total) return [];
    return stops.flatMap((stop, i) => i && ids.includes(stop.point.id)
        ? [{ coordinates: routeSlice(coordinates, stops[i - 1].distance / total, stop.distance / total), km: stop.distance - stops[i - 1].distance, at: stops[i - 1].distance }] : []);
}
