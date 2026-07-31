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

    it("serves schema tooling only with the maintainer editor", () => {
        expect(host.platform.schema !== null).toBe(host.loadStyleEditor !== null);
        expect(host.platform.palette !== null).toBe(host.loadStyleEditor !== null);
    });

});

describe("the hosts as a set", () => {
    it("agrees with the tier split in #894", () => {
        // Product hosts consume published cells; only the dev host loads the
        // maintainer schema editor.
        expect(web.loadStyleEditor).toBeNull();
        expect(desktop.loadStyleEditor).toBeNull();
        expect(dev.loadStyleEditor).not.toBeNull();
        expect(web.platform.caps.rideLibrary).toBe(false);
        // Which is *why* the hosted tier has no schema and no palette: both are
        // absent by design, so neither may become a seam some issue owes.
        expect(web.platform.schema).toBeNull();
        expect(web.platform.palette).toBeNull();
        expect(desktop.platform.caps).toEqual({
            rideLibrary: true,
            deviceUsb: true,
            deviceDashboard: true,
        });
    });

    it("borrows the browser's USB stack only where there is nothing else", () => {
        // Which transport `device()` uses, not whether it has one. The hosted
        // tier's Chromium-only reach is exactly why the desktop app exists
        // (#894), and #901 turns it into its own gate with its own sentence.
        expect(web.platform.usbViaWebUsb).toBe(true);
        expect(desktop.platform.usbViaWebUsb).toBe(false);
        // Never true where there is no USB at all, or the gate would blame a
        // browser for a tier that was never going to connect.
        for (const host of Object.values(HOSTS)) {
            if (!host.platform.caps.deviceUsb) expect(host.platform.usbViaWebUsb).toBe(false);
        }
    });

    it("offers a legacy-config import only on the host that could have one", () => {
        // user_config.json was the retired editor's server-side persistence;
        // only the FastAPI host ever wrote one.
        expect(typeof dev.platform.legacyConfig).toBe("function");
        expect(web.platform.legacyConfig).toBeUndefined();
        expect(desktop.platform.legacyConfig).toBeUndefined();
    });

    it("links back to the site only where there is a site around the app", () => {
        // The desktop app is a standalone window: no docs/simulator/GitHub in
        // its chrome, tabs instead. Optional member, not a cap — a nav link has
        // no moment of intent to gate (#901).
        expect(web.platform.siteNav).toBeDefined();
        expect(dev.platform.siteNav).toBeDefined();
        expect(desktop.platform.siteNav).toBeUndefined();
    });

    it("reports and clears caches only where the app owns a filesystem", () => {
        // The same shape as `legacyConfig`: optional, not a capability, because
        // a tier without a disk has nothing to report and no gate sentence worth
        // writing. The desktop app's caches reach gigabytes (#906), so it does.
        expect(desktop.platform.storage).toBeDefined();
        expect(web.platform.storage).toBeUndefined();
        expect(dev.platform.storage).toBeUndefined();
        // `revealFile` supports the desktop's managed ride files.
        expect(typeof desktop.platform.revealFile).toBe("function");
        expect(web.platform.revealFile).toBeUndefined();
        expect(dev.platform.revealFile).toBeUndefined();
    });
});
