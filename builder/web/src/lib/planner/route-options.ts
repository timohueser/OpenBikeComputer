import { planView, type Trip } from './editor';
import { coordinateAt, cumulative, routeDistance, type Coordinate } from './geo';
import { profileAscent, type ProfileLine } from './profile-data';
import { presetName } from './riding-profiles';
import { unpavedMetres, type EngineRoute, type RoutingLine } from './routing';

/** The rider's current route first, then the other options in the order of the answer. Letter A is the current route. */
export function optionOrder(routes: EngineRoute[], choice: string): EngineRoute[] {
    const current = routes.find(route => route.id === choice);
    return current ? [current, ...routes.filter(route => route !== current)] : routes;
}

export const optionLetter = (index: number) => String.fromCharCode(65 + index);

const reasonNames: Record<string, string> = { shorter: 'Shorter', less_climbing: 'Less climbing', smoother: 'Smoother' };

/** A corridor route takes the settlement near its `via` from `places`, else "Other way"; the primary route its preset. */
export function optionName(route: EngineRoute, places: Record<string, string>): string {
    if (route.reason === 'corridor') return places[route.id] ? `Via ${places[route.id]}` : 'Other way';
    return reasonNames[route.reason] ?? presetName(route.profile);
}

/** One row of a comparison, a route option or a saved version, with the figures that the list compares. */
export interface Compared {
    id: string;
    name: string;
    /** A short line under the name, such as the preset of your route. */
    detail?: string;
    /** The line and its heights, for the map and the profile. */
    line: ProfileLine;
    km: number;
    /** Metres of climb, measured on the heights; null when some heights are unknown, as the climb is then a lower bound. */
    climb: number | null;
    unpavedKm: number;
    unknownSurfaceKm: number;
}

const profiles = new WeakMap<EngineRoute, ProfileLine>();

/** A route of the route service as a row. One line object for each route, so its profile samples are computed once. */
export function routeRow(route: EngineRoute, name: string): Compared {
    let line = profiles.get(route);
    if (!line) profiles.set(route, line = { coordinates: route.geometry, elevation: route.elevation });
    const { totals } = route;
    return { id: route.id, name, line, km: totals.distance_m / 1000, climb: totals.unknown_elevation_m ? null : profileAscent(0, 1, line),
        unpavedKm: unpavedMetres(totals) / 1000, unknownSurfaceKm: totals.surface_m[0] / 1000 };
}

/** A plan and its calculated line as a row, with the distance and climb of the route summary. */
export function lineRow(id: string, name: string, trip: Trip, line: RoutingLine): Compared {
    const { summary } = planView(trip, line);
    return { id, name, line, km: summary.distance, climb: line.unknownElevationKm ? null : summary.ascent, unpavedKm: line.unpavedKm,
        unknownSurfaceKm: line.unknownSurfaceKm };
}

const tenths = (km: number) => Math.abs(km).toFixed(1);

/** How `row` differs from the current route, in words: "5.5 km shorter · 237 m less climbing · 0.1 km more unpaved".
 * Climb stays out when either climb is unknown; unpaved stays out when it rounds to nothing. */
export function difference(row: Compared, current: Compared): string {
    const km = row.km - current.km;
    const parts = [Number(tenths(km)) ? `${tenths(km)} km ${km < 0 ? 'shorter' : 'longer'}` : 'Same distance'];
    if (row.climb !== null && current.climb !== null) {
        const climb = Math.round(row.climb - current.climb);
        parts.push(climb ? `${Math.abs(climb)} m ${climb < 0 ? 'less' : 'more'} climbing` : 'same climbing');
    }
    const unpaved = row.unpavedKm - current.unpavedKm;
    if (Number(tenths(unpaved))) parts.push(`${tenths(unpaved)} km ${unpaved < 0 ? 'less' : 'more'} unpaved`);
    return parts.join(' · ');
}

/** The middle of the longest stretch of `line` more than 50 m from `other`, where the map marks the line with its letter.
 * Undefined when `line` follows `other` all the way. */
export function divergence(line: Coordinate[], other: Coordinate[]): Coordinate | undefined {
    const near = routeDistance(other, .05), km = cumulative(line);
    let best: [number, number] | undefined, from = -1;
    line.forEach((point, i) => {
        const off = near(point) === Infinity;
        if (off && from < 0) from = i;
        if (from < 0 || (off && i < line.length - 1)) return;
        const to = off ? i : i - 1;
        if (!best || km[to] - km[from] > km[best[1]] - km[best[0]]) best = [from, to];
        from = -1;
    });
    return best && coordinateAt(line, (km[best[0]] + km[best[1]]) / 2 / (km.at(-1) || 1));
}
