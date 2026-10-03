import { coordinateAt, cumulative, kilometres, type Coordinate, type RoutePoint } from './editor';

/** A shorter gap is a coarse click, or a building set back from its road: a map pixel at zoom 10 is about 100 m. */
const shortestKm = 0.1;

export interface RouteGap {
    point: RoutePoint;
    /** From the route to the point, where the rider put it. */
    coordinates: [Coordinate, Coordinate];
    km: number;
}

/** The points that the route does not reach, because no accessible road is nearer. The router attaches such a point to
 * the nearest road within 1 km (`specs/route-api.md`). */
export function routeGaps(stops: { point: RoutePoint; distance: number }[], coordinates: Coordinate[]): RouteGap[] {
    const total = cumulative(coordinates).at(-1) ?? 0;
    if (!total) return [];
    return stops.flatMap(({ point, distance }, i) => {
        // A manual leg from the point already joins the route to it.
        if ((stops[i + 1]?.point.leg ?? 'routed') !== 'routed') return [];
        const attached = coordinateAt(coordinates, distance / total);
        const km = kilometres(attached, point.coordinate);
        return km > shortestKm ? [{ point, coordinates: [attached, point.coordinate], km }] : [];
    });
}

/** One quiet line, such as "Route ends 180 m before the selected point — no accessible path". */
export function gapNote(gaps: RouteGap[]): string {
    const km = Math.max(...gaps.map(gap => gap.km));
    const distance = km < 1 ? `${Math.round(km * 100) * 10} m` : `${km.toFixed(1)} km`;
    const kind = gaps[0].point.kind;
    const route = gaps.length > 1 ? `Route misses ${gaps.length} selected points by up to ${distance}`
        : kind === 'start' ? `Route starts ${distance} from the selected point`
        : kind === 'finish' ? `Route ends ${distance} before the selected point`
        : `Route passes ${distance} from the selected point`;
    return `${route} — no accessible path`;
}
