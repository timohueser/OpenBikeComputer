import { expect, it } from "vitest";
import { parseRoute } from "./routes";

it("compares whole segments and takes the rest of the hash as the id", () => {
    expect(parseRoute("")).toEqual({ route: "home" });
    expect(parseRoute("#/")).toEqual({ route: "home" });
    expect(parseRoute("#/routesX")).toEqual({ route: "home" });
    expect(parseRoute("#/routes")).toEqual({ route: "routes" });
    expect(parseRoute("#/route/ID")).toEqual({ route: "route", id: "ID" });
    expect(parseRoute("#/planner/ID")).toEqual({ route: "planner", id: "ID" });
    expect(parseRoute("#/device")).toEqual({ route: "device" });
    expect(parseRoute("#/device/area")).toEqual({ route: "area" });
    expect(parseRoute("#/signed/9-268-180/11680539")).toEqual({ route: "signed", id: "9-268-180/11680539" });
});

it("leaves the parameters after ? to the page", () => {
    expect(parseRoute("#/explore?walk=500&car=1")).toEqual({ route: "explore" });
    expect(parseRoute("#/explore/?car=1")).toEqual({ route: "explore" });
});
