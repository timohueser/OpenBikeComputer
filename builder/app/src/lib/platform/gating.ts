// The gating layer (#901). Desktop-only features stay on screen, disabled,
// each with a one-line reason and a next step — the moment of intent is where
// the explanation belongs, not a marketing page you have to go looking for.
//
// **Two questions, not one.** They look alike and they are not:
//
//   * **Tier** — does this *host* have the feature at all? A `Caps` flag. The
//     remedy is the desktop app.
//   * **Browser** — can the browser running right now reach a USB device? This
//     has nothing to do with the tier. `caps.deviceUsb` is true on the hosted
//     site because WebUSB is that tier's design; it is not a claim about
//     Safari. The remedy is Chrome or Edge — *or* the desktop app, for a
//     different reason than a Firefox user wanting a ride library.
//
// Collapse them and half the visitors get the wrong sentence, so they stay
// apart the whole way down: `Requirement` covers both, `unmetIn` reports
// *which* one failed, and each has its own line in `GATES`.
//
// **One place for the words.** Every reason and every next step is written
// here, once. A component declares the requirement it needs; it never reads
// `platform.caps`, never asks which host it is on, and never writes its own
// copy. `Record<Requirement, Gate>` makes that structural — a new capability
// flag fails to compile until someone has written the sentence that goes with
// it, so nothing can end up disabled with nothing to say.

import { RELEASE } from "../desktop/release";
import { DESKTOP_ROUTE } from "../routes";
import { platform, type Caps } from "./index";

/** A capability of the *host*: absent tiers point at the desktop app. */
export type TierRequirement = keyof Caps;

/** The one gate that is not a tier decision. */
export type BrowserRequirement = "webUsb";

export type Requirement = TierRequirement | BrowserRequirement;

/**
 * Every requirement, in the order the desktop page lists them: what you make,
 * then what you keep, then how you reach the device.
 */
export const REQUIREMENTS: readonly Requirement[] = [
    "rideLibrary",
    "deviceUsb",
    "deviceDashboard",
    "webUsb",
];

export interface Gate {
    /** Shown inline where the control is. One line, present tense, stating
     *  where the feature lives — not apologising that it isn't here. */
    readonly reason: string;
    /** Its heading on the desktop page. */
    readonly title: string;
    /** What the desktop app does instead, for the desktop page's list. */
    readonly offer: string;
}

export const GATES: Record<Requirement, Gate> = {
    rideLibrary: {
        reason: "Your ride library lives in the desktop app.",
        title: "A ride library",
        offer: "Rides pulled off the device land in a folder you pick, with a list, a track preview and GPX export.",
    },
    deviceUsb: {
        reason: "USB transfers live in the desktop app.",
        title: "USB to the device",
        offer: "Maps, routes and firmware updates go over the cable instead of the card.",
    },
    deviceDashboard: {
        reason: "The device dashboard lives in the desktop app.",
        title: "Device dashboard",
        offer: "Battery, card space and every setting the device exposes, edited from the app.",
    },
    webUsb: {
        reason: "Your browser can't talk to USB devices — Chrome and Edge can.",
        title: "USB without Chrome",
        offer: "The app drives USB itself, so Safari and Firefox work the same as Chrome.",
    },
};

/**
 * The next step every gate offers. One link, because the desktop app is the
 * answer to all of them — a Safari visitor's other remedy (switch browser) is
 * in the sentence itself, where it belongs.
 *
 * The label tracks reality: promising a download before D3 (#908) has published
 * one would send people to a page with nothing on it, so until `RELEASE` exists
 * the link says what the page actually holds. It flips itself when D3 lands.
 */
export const DESKTOP_LINK = {
    href: DESKTOP_ROUTE,
    label: RELEASE ? "Get the desktop app →" : "What the desktop app adds →",
};

/**
 * Everything a gate decision depends on. Passed in rather than read from the
 * module so all three tiers — and both answers to the browser question — are
 * reachable from a test without mocking a host.
 */
export interface GateEnv {
    readonly caps: Caps;
    readonly usbViaWebUsb: boolean;
    readonly browserHasUsb: boolean;
}

/** Does the browser running this code expose WebUSB? Chromium yes, Safari and
 *  Firefox no — a fact about the browser, never about the tier. */
export function hasWebUsb(): boolean {
    return typeof navigator !== "undefined" && "usb" in navigator;
}

function satisfied(env: GateEnv, need: Requirement): boolean {
    // A host with its own USB driver is not affected by the browser's answer;
    // one borrowing WebUSB inherits it.
    if (need === "webUsb") return !env.usbViaWebUsb || env.browserHasUsb;
    return env.caps[need];
}

/**
 * The first requirement in `need` this environment does not meet, or null if it
 * meets them all. *First* matters: a USB control declares
 * `["deviceUsb", "webUsb"]`, so a tier that has no USB at all says so, and only
 * a tier that does gets as far as blaming the browser.
 *
 * `value` is the platform member the control needs, such as `platform.device`.
 * A1 pins `caps.X === (member !== null)`, so passing it lets
 * a call site make one check instead of two and hand the narrowed value
 * straight to the control.
 */
export function unmetIn(
    env: GateEnv,
    need: Requirement | readonly Requirement[],
    value?: unknown,
): Requirement | null {
    const needs = typeof need === "string" ? [need] : need;
    for (const one of needs) if (!satisfied(env, one)) return one;
    // Only reachable if a host broke A1's invariant. Report the tier
    // requirement rather than rendering a live control over a null member.
    if (value === null) return needs[0];
    return null;
}

/** What the desktop app adds *for this visitor* — the requirements this
 *  environment does not meet, in `REQUIREMENTS` order. On Chrome that leaves
 *  USB off the list, because on Chrome the site does USB. */
export function desktopAddsIn(env: GateEnv): Requirement[] {
    return REQUIREMENTS.filter((need) => unmetIn(env, need) !== null);
}

/** The environment this page is actually running in. Read once: neither the
 *  host nor the browser's USB support changes while the tab is open. */
export const ENV: GateEnv = {
    caps: platform.caps,
    usbViaWebUsb: platform.usbViaWebUsb,
    browserHasUsb: hasWebUsb(),
};

/** `unmetIn` against the live environment. What `<Gated>` calls. */
export function unmet(
    need: Requirement | readonly Requirement[],
    value?: unknown,
): Requirement | null {
    return unmetIn(ENV, need, value);
}

/** For the handful of places where the honest answer is to show nothing rather
 *  than something disabled — a nav link has no moment of intent to gate. */
export function available(need: Requirement | readonly Requirement[]): boolean {
    return unmet(need) === null;
}

export const DESKTOP_ADDS: Requirement[] = desktopAddsIn(ENV);
