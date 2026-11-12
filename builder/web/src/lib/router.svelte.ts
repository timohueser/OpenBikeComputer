// Hash routing keeps the SPA sub-path tolerant: it works served at "/" or
// under any prefix without server rewrite rules.
//
// The hashes themselves live in `lib/routes.ts`, which has no side effects —
// this module reads `location` and subscribes to `hashchange` on import.

import { ADVANCED_ROUTE } from "./routes";

export type Route = "home" | "advanced";

function parse(): Route {
    if (location.hash.startsWith(ADVANCED_ROUTE)) return "advanced";
    return "home";
}

class Router {
    route = $state<Route>(parse());

    constructor() {
        window.addEventListener("hashchange", () => {
            this.route = parse();
        });
    }
}

export const router = new Router();
