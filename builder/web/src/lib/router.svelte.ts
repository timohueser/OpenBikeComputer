// Hash routing keeps the SPA sub-path tolerant: it works served at "/" or
// under any prefix without server rewrite rules.
//
// The hashes themselves live in `lib/routes.ts`, which has no side effects —
// this module reads `location` and subscribes to `hashchange` on import.

import { parseRoute, ROUTES, type Route } from "./routes";

export type { Route };

class Router {
    route = $state<Route>("home");
    id = $state<string | undefined>();
    /** `#/settings` is a window over the page before it: `route` and `id` keep that page, so it is not created again. */
    settings = $state(false);
    /** The page before the current one in this tab: set when the address names another page or another id. Undefined after
     * a load of the app. */
    from = $state<Route>();

    constructor() {
        // A first address of the settings gets the home page in an entry before it, so each close of the window is Back.
        if (parseRoute(location.hash).route === "settings") {
            const settings = location.hash;
            history.replaceState(history.state, "", ROUTES.home);
            history.pushState(history.state, "", settings);
        }
        this.read(true);
        window.addEventListener("hashchange", () => this.read());
    }

    /** Goes to `hash` in a new entry of the history that holds `state`, for the page there to read. */
    go(hash: string, state: Record<string, unknown>) {
        history.pushState(state, "", hash);
        this.read();
    }

    /** Goes to `hash` in place of the current entry of the history, so Back skips the current address. */
    replace(hash: string) {
        history.replaceState(history.state, "", hash);
        this.read();
    }

    closeSettings() {
        if (this.settings) history.back();
    }

    private read(first = false) {
        const { route, id } = parseRoute(location.hash);
        this.settings = route === "settings";
        if (!this.settings) {
            if (!first && (route !== this.route || id !== this.id)) this.from = this.route;
            this.route = route;
            this.id = id;
        }
    }
}

export const router = new Router();
