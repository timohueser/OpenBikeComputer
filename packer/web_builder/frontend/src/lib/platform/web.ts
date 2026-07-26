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

    regions: () => pending("regions", "C1 #900"),
    presets: () => pending("presets", "B2 #899"),
    schema: () => pending("schema", "C6 #905"),
    palette: () => pending("palette", "C6 #905"),
    catalog: () => pending("catalog", "A3 #897"),

    buildMap: null,
    device: () => pending("device", "C3 #902"),
    rides: null,
};

export const loadStyleEditor: LoadStyleEditor | null = null;
