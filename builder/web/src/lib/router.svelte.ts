// Hash routing keeps the SPA sub-path tolerant: it works served at "/" or
// under any prefix without server rewrite rules.
//
// The hashes themselves live in `lib/routes.ts`, which has no side effects —
// this module reads `location` and subscribes to `hashchange` on import.

import { parseRoute, type Route } from "./routes";

export type { Route };

class Router {
    route = $state<Route>("home");
    id = $state<string | undefined>();

    constructor() {
        this.read();
        window.addEventListener("hashchange", () => this.read());
    }

    /** Goes to `hash` in place of the current entry of the history, so Back skips the current address. */
    replace(hash: string) {
        history.replaceState(history.state, "", hash);
        this.read();
    }

    private read() {
        ({ route: this.route, id: this.id } = parseRoute(location.hash));
    }
}

export const router = new Router();
