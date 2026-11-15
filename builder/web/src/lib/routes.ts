// The hash each route lives at, kept in a module with no side effects:
// `router.svelte.ts` reads `location` and subscribes to `hashchange` the moment
// it is imported, so anything that only needs a URL — a link in a component,
// a node-environment test — imports this instead.

export const ROUTES = {
    home: "#/",
    planner: "#/planner",
    routes: "#/routes",
    route: "#/route",
    ride: "#/ride",
    device: "#/device",
    area: "#/device/area",
    settings: "#/settings",
    advanced: "#/advanced",
} as const;

export type Route = keyof typeof ROUTES;

const segments = (hash: string) => hash.replace(/^#?\/?/, "").split("/").filter(Boolean);
const ROUTE_SEGMENTS = Object.entries(ROUTES).map(([route, hash]) => [route as Route, segments(hash)] as const);

/** The route with the most whole segments at the start of the hash, and the next segment as its id. */
export function parseRoute(hash: string): { route: Route; id?: string } {
    const parts = segments(hash);
    let best: { route: Route; length: number } = { route: "home", length: 0 };
    for (const [route, start] of ROUTE_SEGMENTS) {
        if (start.length > best.length && start.every((segment, i) => parts[i] === segment)) best = { route, length: start.length };
    }
    const id = best.length ? parts[best.length] : undefined;
    return id ? { route: best.route, id } : { route: best.route };
}

/**
 * The firmware card's DOM id — a destination inside a page rather than a page, and the only one
 * the app navigates to on its own, since the update prompt scrolls the rider there. It lives with
 * the routes because it is the same kind of thing: a target two modules have to agree on, spelled
 * once. Not a hash route: the card is a section of the device surfaces, and making it a route
 * would mean two ways to be in the same place.
 */
export const FIRMWARE_ANCHOR = "firmware-card";
