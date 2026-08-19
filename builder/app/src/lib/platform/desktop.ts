// The desktop host: the same catalog builder inside a Tauri shell, with a real
// filesystem and native device access underneath. Every method here is one
// `invoke()` of a Rust command in apps/obc-desktop — D1 (#906) built that
// shell and D4 (#909) added the native USB transport under it.
//
// The caps were already true before any of this was implemented: they are the
// tier's contract, and what C2's gating layer compares the web tier against.
// What D1 changed is that most of them now have something behind them.
//
// Where this host differs from the dev server, it differs because of the two
// things it has that a server does not — a filesystem and native device access:
//
//   * `catalog()` reads the published manifest through Rust rather than
//     `fetch()`. The window is granted no network capability at all, so the set
//     of hosts this app talks to is a reviewable list in one Rust module.
//   * `device()` drives USB itself (`nusb`), because the system webview has no
//     WebUSB — which is also what makes this the only tier a Safari or Firefox
//     user can plug a device into at all.

import type { MapOutputSession, Platform } from "./types";
import { desktop } from "../desktop/invoke";

// Every seam this host declares is implemented now — D1 (#906) filled in the
// data calls, D4 (#909) the device one and E2 (#912) the ride library — so the
// `pending()` helper that named the issue owing each one has nothing left to
// name here, exactly as it does on the web host.

/**
 * The catalog document as the Rust side fetched it. Validation belongs to the
 * catalog client, so the desktop host has the same single root-fetch seam as
 * the web host. Only a fulfilled read is memoized.
 */
let rootInflight: Promise<{ url: string; body: string }> | null = null;

function catalog(): Promise<{ url: string; body: string }> {
    // Start on a promise turn as well as catching a rejected invoke. The real
    // Tauri bridge is async, but tests and alternate transports may reject by
    // throwing synchronously; either kind of failed read must clear the memo.
    rootInflight ??= Promise.resolve().then(() => desktop.catalog()).catch((e: unknown) => {
        rootInflight = null;
        throw e;
    });
    return rootInflight;
}

const catalogFetch: typeof fetch = async (input, init) => {
    if (init?.signal?.aborted) throw init.signal.reason;
    const url = input instanceof Request ? input.url : input.toString();
    const bytes = await desktop.catalogGet(url);
    if (init?.signal?.aborted) throw init.signal.reason;
    return new Response(bytes, { status: 200 });
};

async function openMapOutput(name: string): Promise<MapOutputSession> {
    const opened = await desktop.mapOutputBegin(name);
    return {
        path: opened.path,
        // The IPC takes contiguous bytes; a Blob is read back here, which is the
        // whole map resident — the residency this host has always had, and the
        // reason the assembly's own sink is the saving that matters.
        write: async (filename, body) =>
            desktop.mapOutputWrite(
                opened.id,
                filename,
                body instanceof Uint8Array ? body : new Uint8Array(await body.arrayBuffer()),
            ),
        finish: () => desktop.mapOutputFinish(opened.id),
        discard: () => desktop.mapOutputDiscard(opened.id),
    };
}

export const platform: Platform = {
    name: "desktop",
    caps: {
        rideLibrary: true,
        deviceUsb: true,
        deviceDashboard: true,
    },

    // A native driver (`nusb`, D4 #909), which is the point: the desktop app is
    // the universal USB path, including for the browsers WebUSB never reaches.
    usbViaWebUsb: false,

    catalog,
    catalogFetch,
    openMapOutput,

    styleEditor: null,

    // Native USB (D4 #909), loaded on demand for the same reason the web host
    // does it: the protocol client, the codecs and the transport are their own
    // chunk, and a window that only builds a map never parses them. Underneath
    // it is the same `FlatStoreClient` the browser drives — only the byte pipe differs.
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
};
