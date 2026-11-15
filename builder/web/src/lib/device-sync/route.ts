/** Encodes a plan with its calculated line as one OBCR route for the device, also a plan of several days. */
import { lineToObcr, type LineWaypoint } from '../convert/bridge';
import { dayStops, planTitle, planView, type RoutePoint } from '../planner/editor';
import { coordinateAt } from '../planner/geo';
import type { Plan } from '../planner/library';
import { poiKinds, type PlaceCategory } from '../planner/poi-kinds';
import { SURFACES, type RoutingLine } from '../planner/routing';

/** The device reads this many waypoints of a route (`MAX_WAYPOINTS` of `obc-route`). */
export const MAX_WAYPOINTS = 32;

/** The device numbers of `PoiCategory` (`firmware/obc-formats/src/obcm.rs`); another place is generic, 0. */
const categories: Partial<Record<PlaceCategory, number>> = { water: 1, camp: 2, hotel: 3, shop: 4, food: 4, pharmacy: 5, bike: 6 };
/** The device bike types: on foot is Touring. */
const bikes = { road: 0, gravel: 1, mtb: 2, touring: 3, hiking: 3 };

const micro = (degrees: number) => Math.round(degrees * 1e6);
function categoryOf(point?: RoutePoint): number {
    const kind = point?.placeKind && poiKinds[point.placeKind];
    return kind ? categories[kind.category] ?? 0 : 0;
}

/** The route file of `plan` along `line`, the line that `calculateLine` gave for it. The waypoints are the nights, then the
 * stops in route order, at most `MAX_WAYPOINTS`; `left` counts the others. */
export async function routeObcr(plan: Plan, line: RoutingLine): Promise<{ bytes: Uint8Array; left: number }> {
    const trip = plan.trip, view = planView(trip, line);
    const nights = view.days.slice(0, -1).map((day): LineWaypoint => {
        const [lon, lat] = day.pinned?.coordinate ?? coordinateAt(view.coordinates, day.to);
        return { lon: micro(lon), lat: micro(lat), distanceM: day.to * view.total * 1000, category: categoryOf(day.pinned), name: day.pinned?.label || `Night ${day.number}` };
    });
    const stops = dayStops(trip, line, { from: 0, to: 1 }).map(({ point, km }): LineWaypoint =>
        ({ lon: micro(point.coordinate[0]), lat: micro(point.coordinate[1]), distanceM: km * 1000, category: categoryOf(point), name: point.label }));
    const waypoints = [...nights, ...stops];
    const surfaces = line.edges.surfaces;
    const bytes = await lineToObcr({
        coordinates: Int32Array.from(line.coordinates.flatMap(([lon, lat]) => [micro(lon), micro(lat)])),
        heights: line.elevation,
        surfaces: Uint8Array.from(line.coordinates, (_, i) => Math.max(0, SURFACES.indexOf(surfaces?.[i - 1] ?? 'Unknown'))),
    }, waypoints.slice(0, MAX_WAYPOINTS), plan.name || planTitle(trip), bikes[trip.bike ?? 'touring']);
    return { bytes, left: Math.max(0, waypoints.length - MAX_WAYPOINTS) };
}
