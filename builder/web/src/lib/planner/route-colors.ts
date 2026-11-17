// Each colour stays apart from the others and from the light map under normal vision and the three kinds of colour
// blindness. White text has 4.5:1 or more on each colour but the last, the lavender.
export const ROUTE_COLORS = ['#cc2a93', '#1f5fd6', '#d63a00', '#5b2a9e', '#0d6e74', '#7a0a0a', '#9988dd'];
const LAVENDER = ROUTE_COLORS[6];

/** The light ground under signed lines and their plain pictures in the dark theme: the dark map is too dark for the set. */
export const SIGNED_GROUND = '#f4f2eb';

/** The colour of a signed route. A hash of its OSM relation ID, not its place in a list, keeps the colour of a route
 * in every view and after a rebake of the catalog. */
export function routeColor(id: string | number): string {
    let hash = 0x811c9dc5;
    for (const char of String(id)) hash = Math.imul(hash ^ char.charCodeAt(0), 0x01000193);
    return ROUTE_COLORS[(hash >>> 0) % ROUTE_COLORS.length];
}

/** The text colour on a route colour, at 4.5:1 or more. */
export function onRouteColor(color: string): string {
    return color === LAVENDER ? '#1c1b14' : '#fff';
}
