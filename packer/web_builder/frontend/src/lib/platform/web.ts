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
    // The hosted tier's preset list is the catalog manifest's own `presets[]`
    // (id, name, blurb, preview reference) — A3 owns that format, C1 renders
    // it. B2's demo maps and preview images hang off those entries; they are
    // not what produces the list.
    presets: () => pending("presets", "A3 #897 (first consumed by C1 #900)"),
    catalog: () => pending("catalog", "A3 #897"),

    buildMap: null,
    device: () => pending("device", "C3 #902"),
    rides: null,

    // Both absent by design, not pending: `schema` has no caller without
    // `caps.build` or `caps.styleEditor` and `palette` none without the color
    // picker, and this tier has neither — permanently, because having neither
    // is what makes it serverless. Nothing to serve, and no issue that owes it.
    schema: null,
    palette: null,
};

export const loadStyleEditor: LoadStyleEditor | null = null;
