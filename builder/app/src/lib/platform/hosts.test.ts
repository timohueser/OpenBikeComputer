// The invariant that makes the seam safe (#895): a capability flag and the
// member it gates are the same fact.
//
// The hosts are imported directly, not through `$host`, because the point is
// to check all three in one run.

import { describe, expect, it } from "vitest";
import * as desktop from "./desktop";
import * as dev from "./dev";
import * as web from "./web";
import type { Caps, Platform } from "./types";

const HOSTS = { web, desktop, dev };

/** Every cap that gates a member, paired with the member it gates. */
const GATED: Array<[keyof Caps, (p: Platform) => unknown]> = [
    ["deviceUsb", (p) => p.device],
    ["rideLibrary", (p) => p.rides],
];

describe.each(Object.entries(HOSTS))("%s host", (name, host) => {
    it("names itself", () => {
        expect(host.platform.name).toBe(name);
    });

    it.each(GATED)("has a %s member exactly when the cap says so", (cap, member) => {
        expect(member(host.platform) !== null).toBe(host.platform.caps[cap]);
    });

});

describe("the hosts as a set", () => {
    it("agrees with the tier split in #894", () => {
        // Product hosts consume published cells; only the dev host loads the
        // maintainer schema editor.
        expect(web.platform.styleEditor).toBeNull();
        expect(desktop.platform.styleEditor).toBeNull();
        expect(dev.platform.styleEditor).not.toBeNull();
        expect(web.platform.caps.rideLibrary).toBe(false);
        // Which is *why* the hosted tier has no schema and no palette: both are
        // absent by design, so neither may become a seam some issue owes.
        expect(desktop.platform.caps).toEqual({
            rideLibrary: true,
            deviceUsb: true,
            deviceDashboard: true,
        });
    });

    it("borrows the browser's USB stack only where there is nothing else", () => {
        // Which transport `device()` uses, not whether it has one. Both browser
        // hosts borrow Chromium's WebUSB stack; the desktop app has a native
        // transport. #901 turns the browser limitation into its own gate and
        // its own sentence.
        expect(web.platform.usbViaWebUsb).toBe(true);
        expect(dev.platform.usbViaWebUsb).toBe(true);
        expect(desktop.platform.usbViaWebUsb).toBe(false);
        // Never true where there is no USB at all, or the gate would blame a
        // browser for a tier that was never going to connect.
        for (const host of Object.values(HOSTS)) {
            if (!host.platform.caps.deviceUsb) expect(host.platform.usbViaWebUsb).toBe(false);
        }
    });

    it("links back to the site only where there is a site around the app", () => {
        // The desktop app is a standalone window: no docs/simulator/GitHub in
        // its chrome, tabs instead. Optional member, not a cap — a nav link has
        // no moment of intent to gate (#901).
        expect(web.platform.siteNav).toBeDefined();
        expect(dev.platform.siteNav).toBeDefined();
        expect(desktop.platform.siteNav).toBeUndefined();
    });
});
