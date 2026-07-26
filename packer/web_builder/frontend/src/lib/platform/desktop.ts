// The desktop host: the same frontend inside a Tauri shell, with obc-pack
// linked in as a library and a real filesystem underneath. Every method here
// becomes an `invoke()` of a Rust command — D1 (#906) builds that shell and
// D4 (#909) the native USB transport, so this file is capability declarations
// plus the seams they hang off.
//
// The caps are the tier's contract and are already true: they are what C2's
// gating layer compares the web tier against, and they do not change when the
// implementations land.

import { PlatformNotImplemented, type LoadStyleEditor, type Platform } from "./types";

// `async` so an unimplemented seam *rejects* rather than throwing past the
// caller's `.catch()` — every method here is declared to return a promise, and
// a seam that isn't written yet must not also break that contract.
async function pending(member: string, owner: string): Promise<never> {
    throw new PlatformNotImplemented("desktop", member, owner);
}

export const platform: Platform = {
    name: "desktop",
    caps: {
        build: true,
        bboxCrop: true,
        styleEditor: true,
        rideLibrary: true,
        deviceUsb: true,
        deviceDashboard: true,
    },

    regions: () => pending("regions", "D1 #906"),
    presets: () => pending("presets", "D1 #906"),
    // A3 (#897) settled the manifest format and C1 (#900) reads it; what is
    // still owed here is the desktop shell's own way of fetching it.
    catalog: () => pending("catalog", "D1 #906"),

    // Non-null here, unlike on the web tier: the desktop app builds maps and
    // ships the style editor, so it has both callers and a linked-in obc-pack
    // to answer them. D1 wires them to Tauri commands.
    schema: () => pending("schema", "D1 #906"),
    palette: () => pending("palette", "D1 #906"),

    // The one seam that has to throw synchronously — it hands back a session,
    // not a promise. Better here than on first use: a build that reports
    // progress and only then discovers it has no backend is far worse than one
    // that never starts.
    buildMap: () => {
        throw new PlatformNotImplemented("desktop", "buildMap", "D1 #906");
    },
    device: () => pending("device", "D4 #909"),
    rides: () => pending("rides", "E2 #912"),
};

export const loadStyleEditor: LoadStyleEditor | null = () => import("../../routes/Advanced.svelte");
