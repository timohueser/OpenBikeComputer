// The invariant that makes the seam safe (#895): a capability flag and the
// member it gates are the same fact. If they ever drift, `caps.build` says a
// host can build while `buildMap` is null (dead UI) — or worse, says it can't
// while a callable `buildMap` sits there waiting to be found.
//
// The hosts are imported directly, not through `$host`, because the point is
// to check all three in one run.

import { describe, expect, it } from "vitest";
import * as desktop from "./desktop";
import * as dev from "./dev";
import * as web from "./web";
import { PlatformNotImplemented, type Caps, type Platform } from "./types";

const HOSTS = { web, desktop, dev };

/** Every cap that gates a member, paired with the member it gates. `bboxCrop`
 *  and `deviceDashboard` gate UI only and have nothing to pair with. */
const GATED: Array<[keyof Caps, (p: Platform) => unknown]> = [
    ["build", (p) => p.buildMap],
    ["deviceUsb", (p) => p.device],
    ["rideLibrary", (p) => p.rides],
    ["styleEditor", (p) => p.palette],
];

describe.each(Object.entries(HOSTS))("%s host", (name, host) => {
    it("names itself", () => {
        expect(host.platform.name).toBe(name);
    });

    it.each(GATED)("has a %s member exactly when the cap says so", (cap, member) => {
        expect(member(host.platform) !== null).toBe(host.platform.caps[cap]);
    });

    it("loads the style editor exactly when the cap says so", () => {
        expect(host.loadStyleEditor !== null).toBe(host.platform.caps.styleEditor);
    });

    it("serves a schema exactly when something would read one", () => {
        // The one gate that is a disjunction: the build card and the style
        // editor are `schema`'s only two callers, so a tier with neither has
        // nothing to serve — and never will, which is why it is null and not
        // a PlatformNotImplemented owed by some later issue.
        const { caps, schema } = host.platform;
        expect(schema !== null).toBe(caps.build || caps.styleEditor);
    });

    it("fails unimplemented seams with PlatformNotImplemented, naming its issue", async () => {
        // `catalog()` is the one seam no host implements yet (A3 #897 owns the
        // format), so it is the honest sample of the not-written-yet path.
        await expect(host.platform.catalog()).rejects.toThrow(PlatformNotImplemented);
        await expect(host.platform.catalog()).rejects.toThrow(/A3 #897/);
    });
});

describe("the hosts as a set", () => {
    it("agrees with the tier split in #894", () => {
        // The locked decisions, restated where a change to them fails a test:
        // presets only on the web, style editor and bbox crop desktop-only, and
        // a ride library only where a real folder exists.
        expect(web.platform.caps.build).toBe(false);
        expect(web.platform.caps.styleEditor).toBe(false);
        expect(web.platform.caps.bboxCrop).toBe(false);
        expect(web.platform.caps.rideLibrary).toBe(false);
        // Which is *why* the hosted tier has no schema and no palette: both are
        // absent by design, so neither may become a seam some issue owes.
        expect(web.platform.schema).toBeNull();
        expect(web.platform.palette).toBeNull();
        expect(desktop.platform.caps).toEqual({
            build: true,
            bboxCrop: true,
            styleEditor: true,
            rideLibrary: true,
            deviceUsb: true,
            deviceDashboard: true,
        });
    });

    it("offers a legacy-config import only on the host that could have one", () => {
        // user_config.json was the retired editor's server-side persistence;
        // only the FastAPI host ever wrote one.
        expect(typeof dev.platform.legacyConfig).toBe("function");
        expect(web.platform.legacyConfig).toBeUndefined();
        expect(desktop.platform.legacyConfig).toBeUndefined();
    });
});
