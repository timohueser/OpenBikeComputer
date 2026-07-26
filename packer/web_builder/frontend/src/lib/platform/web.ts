// The static hosted host: files on a CDN and nothing else — no backend at all,
// which is the whole point of the hosted tier (#894). Everything it serves is
// either a baked artifact or something wasm computes in the tab.
//
// The data calls below are seams, not implementations: the artifacts they read
// are produced by the bakery (B1 #898) and their layout is settled when the
// static tier is deployed (C6 #905). Until then they fail loudly rather than
// fetching a URL nobody has decided on yet.

import { PlatformNotImplemented, type LoadStyleEditor, type Platform } from "./types";

// `async` so an unimplemented seam *rejects* rather than throwing past the
// caller's `.catch()` — every method here is declared to return a promise, and
// a seam that isn't written yet must not also break that contract.
async function pending(member: string, owner: string): Promise<never> {
    throw new PlatformNotImplemented("web", member, owner);
}

export const platform: Platform = {
    name: "web",
    caps: {
        // No server, so no obc-pack: the hosted tier serves whole pre-baked
        // regions, and custom maps are what the desktop app is for.
        build: false,
        bboxCrop: false,
        styleEditor: false,
        // A browser ride library would be OPFS/IndexedDB: invisible, evictable
        // and unbackupable. Web exports one GPX and keeps no record.
        rideLibrary: false,
        // WebUSB is this tier's design (Chromium-only, hence the desktop app);
        // C3 #902 is what makes the call below work.
        deviceUsb: true,
        deviceDashboard: false,
    },

    // Chromium-only, and this tier has no other way to reach a cable — so on
    // Safari and Firefox the USB features gate on the *browser*, with their own
    // reason and their own remedy. The download-and-copy-to-the-card path is
    // unaffected and stays open (#901).
    usbViaWebUsb: true,

    regions: () => pending("regions", "C1 #900"),
    // The hosted tier's preset list is the catalog manifest's own `presets[]`
    // (id, name, blurb, preview reference) — A3 owns that format, C1 renders
    // it. B2's demo maps and preview images hang off those entries; they are
    // not what produces the list.
    presets: () => pending("presets", "A3 #897 (first consumed by C1 #900)"),
    catalog: () => pending("catalog", "A3 #897"),

    buildMap: null,
    // WebUSB, loaded on demand. The import is dynamic so the transport, the
    // protocol codecs and the client land in their own chunk: a visitor who only
    // downloads a map never pays for the device stack, and a browser without
    // WebUSB never fetches it at all. The session it returns is `unsupported`
    // there rather than absent — the tier *has* the capability, this browser
    // doesn't, and those are different sentences for the UI to say.
    device: async () => {
        const { openWebUsbSession } = await import("../usb/session.svelte");
        return openWebUsbSession();
    },
    rides: null,

    // Both absent by design, not pending: `schema` has no caller without
    // `caps.build` or `caps.styleEditor` and `palette` none without the color
    // picker, and this tier has neither — permanently, because having neither
    // is what makes it serverless. Nothing to serve, and no issue that owes it.
    schema: null,
    palette: null,
};

export const loadStyleEditor: LoadStyleEditor | null = null;
