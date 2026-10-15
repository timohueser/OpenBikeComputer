import { coordinateAt, cumulative, routeDistance, type Coordinate } from './geo';
import { profileAscent, type ProfileLine } from './profile-data';
import { presetName } from './riding-profiles';
import { SURFACES, type EngineRoute } from './routing';
import { UNPAVED } from './search/segments';

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

export const unpavedKm = (route: EngineRoute) => route.totals.surface_m.reduce((sum, m, i) => UNPAVED.includes(SURFACES[i]) ? sum + m : sum, 0) / 1000;
export const elevationComplete = (route: EngineRoute) => route.totals.unknown_elevation_m === 0;

const profiles = new WeakMap<EngineRoute, ProfileLine>();
/** The heights along `route`, one object for each route, so its profile samples are computed once. */
export function routeProfile(route: EngineRoute): ProfileLine {
    let line = profiles.get(route);
    if (!line) profiles.set(route, line = { coordinates: route.geometry, elevation: route.elevation });
    return line;
}

/** Metres of climb, measured on the heights as the route summary does: the service's total rounds each piece. */
export const climbM = (route: EngineRoute) => profileAscent(0, 1, routeProfile(route));

const km = (metres: number) => (Math.abs(metres) / 1000).toFixed(1);

/** How `route` differs from the current route, in words: "5.5 km shorter · 237 m less climbing · 0.1 km more unpaved".
 * Climb stays out when either route has incomplete elevation, as its ascent is then a lower bound; unpaved stays out
 * when it rounds to nothing. */
export function difference(route: EngineRoute, current: EngineRoute): string {
    const metres = route.totals.distance_m - current.totals.distance_m;
    const parts = [Number(km(metres)) ? `${km(metres)} km ${metres < 0 ? 'shorter' : 'longer'}` : 'Same distance'];
    if (elevationComplete(route) && elevationComplete(current)) {
        const climb = Math.round(climbM(route) - climbM(current));
        parts.push(climb ? `${Math.abs(climb)} m ${climb < 0 ? 'less' : 'more'} climbing` : 'same climbing');
    }
    const unpaved = (unpavedKm(route) - unpavedKm(current)) * 1000;
    if (Number(km(unpaved))) parts.push(`${km(unpaved)} km ${unpaved < 0 ? 'less' : 'more'} unpaved`);
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
