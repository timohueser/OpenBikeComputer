// The desktop host: the same frontend inside a Tauri shell, with obc-pack linked
// in as a library and a real filesystem underneath. Every method here is one
// `invoke()` of a Rust command in firmware/obc-desktop — D1 (#906) built that
// shell, and D4 (#909) still owes the native USB transport.
//
// The caps were already true before any of this was implemented: they are the
// tier's contract, and what C2's gating layer compares the web tier against.
// What D1 changed is that most of them now have something behind them.
//
// Where this host differs from the dev server, it differs because of the two
// things it has that a server does not — a filesystem and the packer in-process:
//
//   * `catalog()` reads the published manifest through Rust rather than
//     `fetch()`. The window is granted no network capability at all, so the set
//     of hosts this app talks to is a reviewable list in one Rust module.
//   * `schema()` comes from the linked packer, not from `obc-pack schema` on a
//     PATH that a shipped app does not have. The editor's capability and the
//     binary that packs are the same artifact, structurally.
//   * `buildMap()` writes into a folder the user can open, and can be cancelled.

import { PlatformNotImplemented, type LoadStyleEditor, type Platform } from "./types";
import { DesktopBuild } from "../desktop/build.svelte";
import { desktop } from "../desktop/invoke";
import { parseCatalog, type Catalog } from "../catalog/manifest";

// `async` so an unimplemented seam *rejects* rather than throwing past the
// caller's `.catch()` — every method here is declared to return a promise, and
// a seam that isn't written yet must not also break that contract.
async function pending(member: string, owner: string): Promise<never> {
    throw new PlatformNotImplemented("desktop", member, owner);
}

/**
 * OBCC §7: the whole body, parsed as one document — the Rust side read it whole
 * for exactly that reason. Preview references resolve against the manifest's own
 * location (§2), which is why the command hands back the URL beside the body.
 */
async function catalog(): Promise<Catalog> {
    const { url, body } = await desktop.catalog();
    const parsed = parseCatalog(body);
    return {
        ...parsed,
        presets: parsed.presets.map((p) =>
            p.preview ? { ...p, preview: new URL(p.preview, url).toString() } : p,
        ),
    };
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

    // A native driver (`nusb`, D4 #909), which is the point: the desktop app is
    // the universal USB path, including for the browsers WebUSB never reaches.
    usbViaWebUsb: false,

    regions: () => desktop.regions(),
    presets: () => desktop.presets(),
    catalog,

    schema: () => desktop.schema(),
    palette: () => desktop.palette(),

    // Synchronous, like every `StartBuild`: it hands back a session, not a
    // promise. The session's own `start()` is what talks to the backend.
    buildMap: () => new DesktopBuild(),
    device: () => pending("device", "D4 #909"),
    rides: () => pending("rides", "E2 #912"),

    storage: {
        places: () => desktop.storagePlaces(),
        clear: (id: string) => desktop.storageClear(id),
    },
    revealFile: (path: string) => desktop.revealFile(path),
};

export const loadStyleEditor: LoadStyleEditor | null = () => import("../../routes/Advanced.svelte");
