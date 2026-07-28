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

});

describe("unimplemented seams", () => {
    // Each still-empty seam names the issue that owes it, so a stack trace says
    // who to chase. The list shrinking is the point: the web host's `catalog()`
    // left it when C1 (#900) implemented it, its `device()` when C3 (#902) did,
    // the desktop host's data calls when D1 (#906) built the Tauri backend, and
    // its `device()` when D4 (#909) put `nusb` under the same seam, and its
    // `rides()` when E2 (#912) built the library — every remaining row is a
    // real debt with an owner.
    it.each([["dev catalog", () => dev.platform.catalog(), /A3 #897/]])(
        "%s rejects with PlatformNotImplemented",
        async (_name, call, owner) => {
            await expect(call()).rejects.toThrow(PlatformNotImplemented);
            await expect(call()).rejects.toThrow(owner);
        },
    );
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
        // `revealFile` answers "where did my map go", and only makes sense where
        // a build result carries a path at all.
        expect(typeof desktop.platform.revealFile).toBe("function");
        expect(web.platform.revealFile).toBeUndefined();
        expect(dev.platform.revealFile).toBeUndefined();
    });
});
