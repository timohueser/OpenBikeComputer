import { coordinateAt, cumulative, routeDistance, type Coordinate } from './geo';
import { presetName } from './riding-profiles';
import type { EngineRoute } from './routing';

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

export const unpavedKm = (route: EngineRoute) => route.totals.surface_m.slice(3).reduce((sum, m) => sum + m, 0) / 1000;
export const elevationComplete = (route: EngineRoute) => route.totals.unknown_elevation_m === 0;

/** A minus sign for less, and ±0 when the difference rounds to nothing. */
export function signed(value: number, digits = 0): string {
    const text = Math.abs(value).toFixed(digits);
    return Number(text) === 0 ? '±0' : `${value < 0 ? '−' : '+'}${text}`;
}

/** How `route` differs from the current route: km, m of climb and km unpaved. Climb is null when either route has
 * incomplete elevation, as its ascent is then a lower bound. */
export function difference(route: EngineRoute, current: EngineRoute): { distance: string; climb: string | null; unpaved: string } {
    const a = current.totals, b = route.totals;
    return {
        distance: signed((b.distance_m - a.distance_m) / 1000, 1),
        climb: elevationComplete(route) && elevationComplete(current) ? signed(b.ascent_m - a.ascent_m) : null,
        unpaved: signed(unpavedKm(route) - unpavedKm(current), 1),
    };
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
