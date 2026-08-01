// The hash each route lives at, kept in a module with no side effects:
// `router.svelte.ts` reads `location` and subscribes to `hashchange` the moment
// it is imported, so anything that only needs a URL — a link in a component,
// the gating layer's next step, a node-environment test — imports this instead.

export const HOME_ROUTE = "";
export const ADVANCED_ROUTE = "#/advanced";

/** The desktop download page. The one next step every gate offers, so it is
 *  also the constant sibling features link to rather than spelling out. */
export const DESKTOP_ROUTE = "#/desktop";

/** The device page — what is on the card. Tiers with `caps.deviceDashboard`. */
export const DEVICE_ROUTE = "#/device";

/** The ride library — the managed folder. Tiers with `caps.rideLibrary`. */
export const RIDES_ROUTE = "#/rides";

/**
 * The firmware card's DOM id — a destination inside a page rather than a page, and the only one
 * the app navigates to on its own (#1002's update prompt scrolls the rider there). It lives with
 * the routes because it is the same kind of thing: a target two modules have to agree on, spelled
 * once. Not a hash route: the card is a section of whichever page carries the device surfaces, and
 * making it a route would mean two ways to be in the same place.
 */
export const FIRMWARE_ANCHOR = "firmware-card";
