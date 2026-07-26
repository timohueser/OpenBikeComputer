// The hash each route lives at, kept in a module with no side effects:
// `router.svelte.ts` reads `location` and subscribes to `hashchange` the moment
// it is imported, so anything that only needs a URL — a link in a component,
// the gating layer's next step, a node-environment test — imports this instead.

export const HOME_ROUTE = "";
export const ADVANCED_ROUTE = "#/advanced";

/** The desktop download page. The one next step every gate offers, so it is
 *  also the constant sibling features link to rather than spelling out. */
export const DESKTOP_ROUTE = "#/desktop";
