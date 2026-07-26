// #901's three promises, held where they can't quietly stop being true:
//
//   1. Nothing is disabled without a reason and a next step.
//   2. The tier question and the browser question stay separate — a Safari
//      visitor and a Firefox user wanting a ride library get different
//      sentences, because they have different problems.
//   3. The copy stays plain: no apology, no exclamation, one line.
//
// The hosts are imported directly, like hosts.test.ts, so all three tiers and
// both answers to the browser question are reachable without mocking anything.

import { describe, expect, it } from "vitest";
import * as desktop from "./desktop";
import * as dev from "./dev";
import * as web from "./web";
import {
    desktopAddsIn,
    GATES,
    hasWebUsb,
    REQUIREMENTS,
    unmetIn,
    type GateEnv,
    type Requirement,
} from "./gating";
import type { Platform } from "./types";

/** A host as the gating layer sees it, with the browser's answer supplied. */
function envOf(p: Platform, browserHasUsb: boolean): GateEnv {
    return { caps: p.caps, usbViaWebUsb: p.usbViaWebUsb, browserHasUsb };
}

const chromium = envOf(web.platform, true);
const safari = envOf(web.platform, false);
const desktopApp = envOf(desktop.platform, false);
const devServer = envOf(dev.platform, true);

describe("the reason table", () => {
    it("covers every requirement, in a listed order", () => {
        expect([...REQUIREMENTS].sort()).toEqual(Object.keys(GATES).sort());
    });

    it("has a reason for every capability flag", () => {
        // `Record<Requirement, Gate>` already forces this at compile time; this
        // is the runtime half, and it fails on the host whose caps grew.
        for (const cap of Object.keys(web.platform.caps)) {
            expect(GATES).toHaveProperty(cap);
        }
    });

    it.each(REQUIREMENTS)("says %s in one plain line", (need) => {
        const { reason, title, offer } = GATES[need];
        expect(reason.length).toBeLessThanOrEqual(90);
        expect(reason.endsWith(".")).toBe(true);
        expect(reason).not.toContain("\n");
        // The house voice, as a guard rather than a review note: no apology,
        // no exclamation, no hedging.
        expect(reason).not.toMatch(/!|sorry|unfortunately|afraid|please|oops/i);
        expect(offer).not.toMatch(/!|sorry|unfortunately|afraid|please|oops/i);
        expect(title).toBeTruthy();
        expect(offer).toBeTruthy();
    });
});

describe("tier gating", () => {
    it("blocks the locked web-tier features and names which one", () => {
        expect(unmetIn(chromium, "build")).toBe("build");
        expect(unmetIn(chromium, "bboxCrop")).toBe("bboxCrop");
        expect(unmetIn(chromium, "styleEditor")).toBe("styleEditor");
        expect(unmetIn(chromium, "rideLibrary")).toBe("rideLibrary");
        expect(unmetIn(chromium, "deviceDashboard")).toBe("deviceDashboard");
    });

    it("blocks nothing in the desktop app", () => {
        for (const need of REQUIREMENTS) expect(unmetIn(desktopApp, need)).toBeNull();
    });

    it("lets the dev server build and style, but not reach a device", () => {
        expect(unmetIn(devServer, "build")).toBeNull();
        expect(unmetIn(devServer, "styleEditor")).toBeNull();
        expect(unmetIn(devServer, "deviceUsb")).toBe("deviceUsb");
    });

    it("reports the first unmet requirement, so the sentence matches the cause", () => {
        expect(unmetIn(devServer, ["deviceUsb", "webUsb"])).toBe("deviceUsb");
    });

    it("blocks a control whose platform member is null even where the cap says yes", () => {
        // Only reachable if a host breaks A1's `caps.X === (member !== null)`
        // invariant — better a reason on screen than a live control over null.
        expect(unmetIn(desktopApp, "build", null)).toBe("build");
        expect(unmetIn(desktopApp, "build", () => undefined)).toBeNull();
        // `undefined` means "this gate has no member", not "the member is
        // missing" — otherwise every value-less gate would fail.
        expect(unmetIn(desktopApp, "build", undefined)).toBeNull();
    });
});

describe("the browser question is not the tier question", () => {
    it("passes USB on the hosted tier in Chromium", () => {
        expect(unmetIn(chromium, ["deviceUsb", "webUsb"])).toBeNull();
    });

    it("blames the browser, not the tier, in Safari", () => {
        // The tier is perfectly willing — `caps.deviceUsb` is true on the
        // hosted site because WebUSB is its design. What's missing is WebUSB.
        expect(web.platform.caps.deviceUsb).toBe(true);
        expect(unmetIn(safari, ["deviceUsb", "webUsb"])).toBe("webUsb");
        expect(GATES.webUsb.reason).toMatch(/Chrome and Edge/);
    });

    it("ignores the browser where the host drives USB itself", () => {
        // The Tauri webview is WKWebView on macOS and WebKitGTK on Linux —
        // neither has WebUSB, and neither matters, because `nusb` does the
        // talking. A browser probe alone would have gated this off.
        expect(desktop.platform.usbViaWebUsb).toBe(false);
        expect(unmetIn(desktopApp, "webUsb")).toBeNull();
    });

    it("does not claim WebUSB in a plain node environment", () => {
        expect(hasWebUsb()).toBe(false);
    });
});

describe("what the desktop page lists", () => {
    it("is what this visitor is actually missing", () => {
        expect(desktopAddsIn(chromium)).toEqual([
            "build",
            "bboxCrop",
            "styleEditor",
            "rideLibrary",
            "deviceDashboard",
        ]);
    });

    it("gains the USB line exactly when the browser can't do it", () => {
        expect(desktopAddsIn(chromium)).not.toContain<Requirement>("webUsb");
        expect(desktopAddsIn(safari)).toContain<Requirement>("webUsb");
    });

    it("is empty in the desktop app, which is why nothing links there", () => {
        expect(desktopAddsIn(desktopApp)).toEqual([]);
    });
});
