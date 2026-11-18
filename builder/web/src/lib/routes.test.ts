import { expect, it } from "vitest";
import { parseRoute } from "./routes";

it("compares whole segments and takes the next segment as the id", () => {
    expect(parseRoute("")).toEqual({ route: "home" });
    expect(parseRoute("#/")).toEqual({ route: "home" });
    expect(parseRoute("#/routesX")).toEqual({ route: "home" });
    expect(parseRoute("#/routes")).toEqual({ route: "routes" });
    expect(parseRoute("#/route/ID")).toEqual({ route: "route", id: "ID" });
    expect(parseRoute("#/planner/ID")).toEqual({ route: "planner", id: "ID" });
    expect(parseRoute("#/device")).toEqual({ route: "device" });
    expect(parseRoute("#/device/area")).toEqual({ route: "area" });
});
