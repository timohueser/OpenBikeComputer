import { maxRidingDays, orderedRoutePoints, routingKey, type RoutePoint, type Trip } from './editor';
import type { Coordinate } from './geo';
import { ridingProfiles } from './riding-profiles';

function record(value: unknown): value is Record<string, unknown> {
    return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function finite(value: unknown): value is number {
    return typeof value === 'number' && Number.isFinite(value);
}

function integer(value: unknown, min: number, max: number): value is number {
    return finite(value) && Number.isInteger(value) && value >= min && value <= max;
}

function coordinate(value: unknown): value is Coordinate {
    return Array.isArray(value) && value.length === 2 && value.every(finite) && Math.abs(value[0]) <= 180 && Math.abs(value[1]) <= 90;
}

function point(value: unknown): value is RoutePoint {
    return record(value) && typeof value.id === 'string' && value.id.length > 0 && typeof value.label === 'string'
        && coordinate(value.coordinate) && finite(value.progress) && value.progress >= 0 && value.progress <= 1
        && typeof value.kind === 'string' && ['start', 'finish', 'pass', 'via', 'waypoint', 'detour', 'night', 'marker'].includes(value.kind)
        && (value.leg === undefined || (typeof value.leg === 'string' && ['routed', 'straight', 'drawn', 'transfer'].includes(value.leg)))
        && (value.drawn === undefined || (Array.isArray(value.drawn) && value.drawn.every(c =>
            Array.isArray(c) && (coordinate(c) || (c.length === 3 && coordinate(c.slice(0, 2)) && finite(c[2]))))))
        && (value.anchor === undefined || coordinate(value.anchor))
        && (value.placeKind === undefined || typeof value.placeKind === 'string')
        && (value.autoLabel === undefined || typeof value.autoLabel === 'boolean')
        && (value.turnaround === undefined || value.turnaround === true)
        && (value.note === undefined || typeof value.note === 'string');
}

/** Browser records and imported files must satisfy the route model. */
export function isTrip(value: unknown): value is Trip {
    if (!record(value) || !integer(value.days, 1, maxRidingDays)
        || !Array.isArray(value.points) || !value.points.every(point)
        || typeof value.budget !== 'string' || !['days', 'distance', 'hours'].includes(value.budget)
        || typeof value.variant !== 'string' || !['valley', 'direct'].includes(value.variant)
        || !finite(value.target) || value.target < 1 || !finite(value.limit) || value.limit < 0
        || (value.climbTarget !== undefined && (!finite(value.climbTarget) || value.climbTarget < 0))
        || (value.mode !== undefined && value.mode !== 'route' && value.mode !== 'trip')
        || (value.live !== undefined && typeof value.live !== 'boolean')
        || (value.name !== undefined && typeof value.name !== 'string')
        || (value.startDate !== undefined && (typeof value.startDate !== 'string' || !/^\d{4}-\d{2}-\d{2}$/.test(value.startDate)
            || !Number.isFinite(Date.parse(value.startDate)) || new Date(value.startDate).toISOString().slice(0, 10) !== value.startDate))
        || (value.loop !== undefined && value.loop !== true)) return false;

    const { points, days } = value;
    const ids = new Set(points.map(point => point.id));
    const nights = points.filter(point => point.kind === 'night');
    const route = points.filter(point => point.kind !== 'marker');
    const starts = route.filter(point => point.kind === 'start').length;
    const finishes = route.filter(point => point.kind === 'finish').length;
    if (ids.size !== points.length || starts > 1 || finishes > 1
        || (value.loop ? route.length < 2 || starts !== 1 || finishes !== 0
            : route.length >= 2 ? starts !== 1 || finishes !== 1 : starts + finishes !== route.length)
        || nights.some(point => !integer(point.night, 1, days - 1) || point.id !== `night-${point.night}`)
        || new Set(nights.map(point => point.night)).size !== nights.length) return false;

    if (value.routeOrder !== undefined && (!Array.isArray(value.routeOrder)
        || !value.routeOrder.every(id => typeof id === 'string' && ids.has(id))
        || new Set(value.routeOrder).size !== value.routeOrder.length)) return false;
    if (value.restAfter !== undefined && (!Array.isArray(value.restAfter) || !value.restAfter.every(after => integer(after, 1, days)))) return false;
    if (value.restNames !== undefined && (!Array.isArray(value.restNames) || !value.restNames.every(name => typeof name === 'string'))) return false;
    if (value.splits !== undefined && (!record(value.splits) || !Object.entries(value.splits).every(([night, progress]) =>
        integer(Number(night), 1, days - 1) && finite(progress) && progress >= 0 && progress <= 1))) return false;

    if (value.bike !== undefined && (typeof value.bike !== 'string' || !Object.hasOwn(ridingProfiles, value.bike))) return false;
    const bike = (value.bike ?? 'touring') as keyof typeof ridingProfiles;
    return value.preset === undefined || (typeof value.preset === 'string' && ridingProfiles[bike].presets.includes(value.preset));
}

function routing(value: unknown, trip: Trip): boolean {
    const points = orderedRoutePoints(trip);
    if (!record(value) || value.key !== routingKey(trip) || typeof value.choiceId !== 'string' || typeof value.profile !== 'string'
        || !Array.isArray(value.coordinates) || value.coordinates.length < 2 || !value.coordinates.every(coordinate)
        || !Array.isArray(value.elevation) || value.elevation.length !== value.coordinates.length || !value.elevation.every(h => h === null || finite(h))
        || !Array.isArray(value.elapsed) || value.elapsed.length !== value.coordinates.length
        || !value.elapsed.every((t, i, times) => finite(t) && t >= 0 && (!i || t >= times[i - 1]))
        || points.length < 2 || !Array.isArray(value.stops) || value.stops.length !== points.length
        || !value.stops.every((stop, i, stops) => record(stop) && stop.id === points[i].id
            && finite(stop.distance) && stop.distance >= 0 && (i ? stop.distance >= stops[i - 1].distance : stop.distance === 0))
        || !['seconds', 'unknownSurfaceKm', 'pushingKm', 'unroutedKm', 'unknownElevationKm'].every(key => finite(value[key]) && value[key] >= 0)
        || (value.picked !== undefined && typeof value.picked !== 'boolean') || !record(value.edges)) return false;
    const coordinates = value.coordinates as Coordinate[];
    if (value.stops.at(-1)!.distance === 0 && coordinates.some(p =>
        p[0] !== coordinates[0][0] || p[1] !== coordinates[0][1])) return false;
    const edges = value.edges;
    const closures = ['permit', 'private', 'farm', 'sidepath', 'discouraged', 'limited', 'seasonal', 'conditional', 'unclear'];
    return Object.entries(edges).every(([channel, values]) => Array.isArray(values) && values.length === coordinates.length - 1
        && values.every(v => v === null || (channel === 'surfaces' ? ['Unknown', 'Paved', 'Compacted', 'Gravel', 'Dirt', 'Rough'].includes(v)
            : channel === 'pushing' ? typeof v === 'boolean'
            : channel === 'sac_scale' ? integer(v, 0, 6)
            : channel === 'closures' ? Array.isArray(v) && v.every(c => record(c) && typeof c.kind === 'string' && closures.includes(c.kind) && typeof c.condition === 'string') : true)));
}

/** An invalid cached line is recalculated. A valid picked alternative keeps its geometry. */
export function storedTrip(value: unknown): Trip | undefined {
    if (!isTrip(value)) return undefined;
    if (value.routing === undefined) return value;
    const { routing: _, ...plan } = value;
    return routing(value.routing, value) ? { ...plan, routing: { ...value.routing, package: undefined, alternatives: [], alternativesReady: false } } : plan;
}
