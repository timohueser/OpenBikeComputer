// Hash routing keeps the SPA sub-path tolerant: it works served at "/" or
// under any prefix without server rewrite rules. Two routes are all we need.

export type Route = "home" | "advanced";

function parse(): Route {
    return location.hash.startsWith("#/advanced") ? "advanced" : "home";
}

class Router {
    route = $state<Route>(parse());

    constructor() {
        window.addEventListener("hashchange", () => {
            this.route = parse();
        });
    }

    go(route: Route) {
        location.hash = route === "home" ? "" : "#/advanced";
    }
}

export const router = new Router();
