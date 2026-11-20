/** Encodes a plan with its calculated line as OBCR routes for the device: one route for a plan of one riding day, else one
 * route for each riding day. */
import { lineToObcr, type LineWaypoint } from '../convert/bridge';
import { DISPLAY_NAME_MAX } from '../device/write';
import { truncateUtf8 } from '../format';
import { dayStops, partDays, planTitle, planView, trainAfter, type RoutePoint } from '../planner/editor';
import { cumulative, firstIndex, type Coordinate } from '../planner/geo';
import type { Plan } from '../planner/library';
import { poiKinds, type PlaceCategory } from '../planner/poi-kinds';
import { SURFACES, type RoutingLine, type Surface } from '../planner/routing';

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

/** A route for the device: its OBCR name, its file, and the number of its stops past `MAX_WAYPOINTS`. */
export interface DeviceRoute {
    name: string;
    bytes: Uint8Array;
    left: number;
}

/** The routes of `plan` along `line`, the line that `calculateLine` gave for it, in ride order. A plan of one riding day
 * is one route with the plan name. Else each riding day is a route "Day k ‹place›", with the label of its kept night as the
 * place; the last day and a day with no kept night are "Day k". k counts the riding days of its part (`partDays`), as the
 * device numbers them. A day after a train or bus starts where it arrives, as the phone cuts; a transfer inside a day
 * stays a straight segment. The waypoints are the stops of the day, at most `MAX_WAYPOINTS`, with distances from the
 * start of its route. */
export async function dayRoutes(plan: Plan, line: RoutingLine): Promise<DeviceRoute[]> {
    const trip = plan.trip, { days, stops, total } = planView(trip, line), last = days.length - 1;
    return Promise.all(days.map(async (day, i) => {
        const night = stops.findIndex(stop => stop.point.id === `night-${i}`);
        const from = i > 0 && trainAfter(trip, i) ? stops[night + 1].distance : day.from * total, transfer = from - day.from * total;
        const waypoints = dayStops(trip, line, day).map(({ point, km }): LineWaypoint => ({ lon: micro(point.coordinate[0]),
            lat: micro(point.coordinate[1]), distanceM: Math.max(0, km - transfer) * 1000, category: categoryOf(point), name: point.label }));
        const place = i < last ? day.pinned?.label : undefined;
        const name = !last ? plan.name || planTitle(trip) : truncateUtf8(place ? `Day ${i % partDays + 1} ${place}` : `Day ${i % partDays + 1}`, DISPLAY_NAME_MAX).trimEnd();
        const part = cut(line, from, day.to * total);
        const bytes = await lineToObcr({
            coordinates: Int32Array.from(part.coordinates.flatMap(([lon, lat]) => [micro(lon), micro(lat)])),
            heights: part.heights,
            surfaces: Uint8Array.from(part.surfaces, surface => Math.max(0, SURFACES.indexOf(surface ?? 'Unknown'))),
        }, waypoints.slice(0, MAX_WAYPOINTS), name, bikes[trip.bike ?? 'touring']);
        return { name, bytes, left: Math.max(0, waypoints.length - MAX_WAYPOINTS) };
    }));
}

/** The part of `line` from `from` to `to` kilometres. Each point has its height, and the surface of the segment that ends
 * at it. An end point inside a segment has the surface of that segment and a height between the two heights of it. */
export function cut(line: RoutingLine, from: number, to: number): { coordinates: Coordinate[]; heights: (number | null)[]; surfaces: (Surface | null)[] } {
    const lengths = cumulative(line.coordinates), surfaces = line.edges.surfaces;
    // A day end comes as a share of the whole line. Within 1 mm of a point of the line, such as a kept night, it is the
    // nearest point, so a change elsewhere in the plan leaves the file of the day as it is.
    const snap = (km: number) => {
        const k = firstIndex(lengths.length, v => lengths[v] >= km), gap = (v: number) => Math.abs((lengths[v] ?? Infinity) - km);
        const near = gap(k - 1) <= gap(k) ? k - 1 : k;
        return gap(near) < 1e-6 ? lengths[near] : km;
    };
    [from, to] = [snap(from), snap(to)];
    const segment = (km: number) => Math.min(lengths.length - 2, Math.max(0, firstIndex(lengths.length, k => lengths[k] > km) - 1));
    const at = (j: number, km: number) => {
        const t = Math.max(0, Math.min(1, (km - lengths[j]) / (lengths[j + 1] - lengths[j] || 1))), a = line.elevation[j], b = line.elevation[j + 1];
        return { coordinate: line.coordinates[j].map((n, axis) => n + (line.coordinates[j + 1][axis] - n) * t) as Coordinate,
            height: t === 0 ? a : t === 1 ? b : a === null || b === null ? null : a + (b - a) * t, surface: surfaces?.[j] ?? null };
    };
    const first = segment(from), end = segment(to), start = at(first, from), finish = at(end, to);
    const inside = Array.from({ length: Math.max(0, end - first) }, (_, k) => first + 1 + k).filter(v => lengths[v] < to);
    return {
        coordinates: [start.coordinate, ...inside.map(v => line.coordinates[v]), finish.coordinate],
        heights: [start.height, ...inside.map(v => line.elevation[v]), finish.height],
        // No segment ends at the first point.
        surfaces: [null, ...inside.map(v => surfaces?.[v - 1] ?? null), finish.surface],
    };
}
