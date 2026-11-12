// The hash each route lives at, kept in a module with no side effects:
// `router.svelte.ts` reads `location` and subscribes to `hashchange` the moment
// it is imported, so anything that only needs a URL — a link in a component,
// a node-environment test — imports this instead.

export const ADVANCED_ROUTE = "#/advanced";

/**
 * The firmware card's DOM id — a destination inside a page rather than a page, and the only one
 * the app navigates to on its own, since the update prompt scrolls the rider there. It lives with
 * the routes because it is the same kind of thing: a target two modules have to agree on, spelled
 * once. Not a hash route: the card is a section of the device surfaces, and making it a route
 * would mean two ways to be in the same place.
 */
export const FIRMWARE_ANCHOR = "firmware-card";
