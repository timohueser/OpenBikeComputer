// The desktop host: the same frontend inside a Tauri shell, with obc-pack linked
// in as a library and a real filesystem underneath. Every method here is one
// `invoke()` of a Rust command in apps/obc-desktop — D1 (#906) built that
// shell and D4 (#909) added the native USB transport under it.
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
//   * `device()` drives USB itself (`nusb`), because the system webview has no
//     WebUSB — which is also what makes this the only tier a Safari or Firefox
//     user can plug a device into at all.

import type { LoadStyleEditor, Platform } from "./types";
import { DesktopBuild } from "../desktop/build.svelte";
import { desktop } from "../desktop/invoke";
import { parseCatalog, type Catalog } from "../catalog/manifest";

// Every seam this host declares is implemented now — D1 (#906) filled in the
// data calls, D4 (#909) the device one and E2 (#912) the ride library — so the
// `pending()` helper that named the issue owing each one has nothing left to
// name here, exactly as it does on the web host.

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

    // Native USB (D4 #909), loaded on demand for the same reason the web host
    // does it: the protocol client, the codecs and the transport are their own
    // chunk, and a window that only builds a map never parses them. Underneath
    // it is C3's `ProtocolClient` unchanged — only the byte pipe differs.
    device: async () => {
        const { openNativeSession } = await import("../desktop/usb.svelte");
        return openNativeSession();
    },
    // The managed ride library (E2 #912), loaded on demand for the same reason
    // `device()` is: it reaches the ride codecs, the wasm GPX exporter and the
    // Tauri ride commands, and a window that only builds a map never needs any
    // of it. `platform/bundle.test.ts` pins the module to this target.
    rides: async () => {
        const { openRideLibrary } = await import("../desktop/library");
        return openRideLibrary();
    },

    storage: {
        places: () => desktop.storagePlaces(),
        clear: (id: string) => desktop.storageClear(id),
    },
    // `<a download>` is inert in this webview, so an export is a Rust write —
    // see `Platform.saveText`.
    saveText: (name: string, text: string) => desktop.saveStyle(name, text),
    revealFile: (path: string) => desktop.revealFile(path),
};

export const loadStyleEditor: LoadStyleEditor | null = () => import("../../routes/Advanced.svelte");
