import { emptyTrip, orderedRoutePoints, routingKey, type Trip } from '../../src/lib/planner/editor';
import { cumulative, type Coordinate } from '../../src/lib/planner/geo';
import type { RoutingLine } from '../../src/lib/planner/routing';

/** A three-day trip from Basel to Thun, about 89 km, with no point between its start and finish. It runs along a
 * meridian, so a fraction of its straight line is the same fraction of its kilometres. */
export function testTrip(): Trip {
    return { ...emptyTrip('trip'), points: [
        { id: 'start', kind: 'start', label: 'Basel', coordinate: [7.6, 47.56] },
        { id: 'finish', kind: 'finish', label: 'Thun', coordinate: [7.6, 46.76] },
    ] };
}

/** `trip` with its calculated line: straight from point to point through each drawn vertex, at 15 km/h. */
export function routed(trip: Trip): Trip {
    const points = orderedRoutePoints(trip);
    const coordinates: Coordinate[] = [points[0].coordinate];
    const ends = [0];
    for (const point of points.slice(1)) {
        coordinates.push(...(point.leg === 'drawn' ? point.drawn ?? [] : []).map((c): Coordinate => [c[0], c[1]]), point.coordinate);
        ends.push(coordinates.length - 1);
    }
    const km = cumulative(coordinates);
    const stops = points.map((point, i) => ({ id: point.id, distance: km[ends[i]] }));
    const routing: RoutingLine = { key: routingKey(trip), choiceId: 'test', profile: 'touring', coordinates, stops,
        elevation: coordinates.map(() => null), elapsed: km.map(d => d * 240), seconds: km.at(-1)! * 240, edges: {},
        alternatives: [], alternativesReady: true, unknownSurfaceKm: 0, pushingKm: 0, unroutedKm: 0, unknownElevationKm: 0 };
    return { ...trip, routing };
}
