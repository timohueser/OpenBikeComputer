import { maxRidingDays, type Coordinate, type RoutePoint, type Trip } from './editor';
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
        && (value.leg === undefined || (typeof value.leg === 'string' && ['routed', 'straight', 'drawn'].includes(value.leg)))
        && (value.drawn === undefined || (Array.isArray(value.drawn) && value.drawn.every(coordinate)));
}

/** Storage crosses a trust boundary: both drafts and versions must satisfy the route model. */
export function isTrip(value: unknown): value is Trip {
    if (!record(value) || !integer(value.days, 1, maxRidingDays)
        || !Array.isArray(value.points) || !value.points.every(point)
        || typeof value.budget !== 'string' || !['days', 'distance', 'hours'].includes(value.budget)
        || typeof value.variant !== 'string' || !['valley', 'direct'].includes(value.variant)
        || !finite(value.target) || value.target < 1 || !finite(value.limit) || value.limit < 0
        || (value.climbTarget !== undefined && (!finite(value.climbTarget) || value.climbTarget < 0))
        || (value.mode !== undefined && value.mode !== 'route' && value.mode !== 'trip')) return false;

    const { points, days } = value;
    const ids = new Set(points.map(point => point.id));
    const nights = points.filter(point => point.kind === 'night');
    if (ids.size !== points.length || points.filter(point => point.kind === 'start').length !== 1
        || points.filter(point => point.kind === 'finish').length !== 1
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
