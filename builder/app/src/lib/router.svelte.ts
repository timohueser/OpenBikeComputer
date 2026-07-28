// Hash routing keeps the SPA sub-path tolerant: it works served at "/" or
// under any prefix without server rewrite rules. Five routes are all we need.
//
// The hashes themselves live in `lib/routes.ts`, which has no side effects —
// this module reads `location` and subscribes to `hashchange` on import.

import { ADVANCED_ROUTE, DESKTOP_ROUTE, DEVICE_ROUTE, HOME_ROUTE, RIDES_ROUTE } from "./routes";

export type Route = "home" | "advanced" | "desktop" | "device" | "rides";

const HASH: Record<Route, string> = {
    home: HOME_ROUTE,
    advanced: ADVANCED_ROUTE,
    desktop: DESKTOP_ROUTE,
    device: DEVICE_ROUTE,
    rides: RIDES_ROUTE,
};

function parse(): Route {
    if (location.hash.startsWith(ADVANCED_ROUTE)) return "advanced";
    // "#/device" before "#/desktop" would also work — `startsWith` on the full
    // hash never confuses the two, but keep them adjacent so nobody wonders.
    if (location.hash.startsWith(DESKTOP_ROUTE)) return "desktop";
    if (location.hash.startsWith(DEVICE_ROUTE)) return "device";
    if (location.hash.startsWith(RIDES_ROUTE)) return "rides";
    return "home";
}

class Router {
    route = $state<Route>(parse());

    constructor() {
        window.addEventListener("hashchange", () => {
            this.route = parse();
        });
    }

    go(route: Route) {
        location.hash = HASH[route];
    }
}

export const router = new Router();
