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
    /** A navigation in this document opened the window, so its close goes back to that page. */
    private settingsHasPage = false;

    constructor() {
        this.read(false);
        window.addEventListener("hashchange", () => this.read(true));
    }

    /** Goes to `hash` in place of the current entry of the history, so Back skips the current address. */
    replace(hash: string) {
        history.replaceState(history.state, "", hash);
        this.read(false);
    }

    closeSettings() {
        if (!this.settings) return;
        if (this.settingsHasPage) history.back();
        else this.replace(ROUTES.home);
    }

    private read(navigated: boolean) {
        const { route, id } = parseRoute(location.hash);
        if (route === "settings" && !this.settings) this.settingsHasPage = navigated;
        this.settings = route === "settings";
        if (!this.settings) {
            this.route = route;
            this.id = id;
        }
    }
}

export const router = new Router();
