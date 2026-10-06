// The static hosted host: files on a CDN and nothing else — no backend at all,
// which is the whole point of the hosted tier. Everything it serves is either a
// baked artifact or something wasm computes in the tab.
//
// The host fetches the OBCC manifest of the active planner release, which the
// planner catalogue names at page load, so a data release needs no site build.
// `VITE_CATALOG_URL` overrides it with another manifest, such as a local bake.

import { LINKS } from "../constants";
import { activeRelease } from "../planner/release";
import type { Platform } from "./types";

// `||`, not `??`: an unset variable can arrive as an empty string.
const CATALOG_URL: string | undefined = import.meta.env.VITE_CATALOG_URL || undefined;

/** Absolute URL of a static document, so a relative override resolves against the
 *  page rather than the module. */
function resolve(url: string): string {
    return new URL(url, document.baseURI).toString();
}

async function get(url: string, refresh: boolean): Promise<Response> {
    const res = await fetch(resolve(url), refresh ? { cache: "no-cache" } : undefined);
    if (!res.ok) throw new Error(`${url}: ${res.status} ${res.statusText}`);
    return res;
}

/**
 * The root document as fetched. Validation belongs to `CatalogClient`, not the
 * host seam, so there is one request surface and one parser.
 */
let rootInflight: Promise<{ url: string; body: string }> | null = null;

function fetchCatalog({ refresh = false }: { refresh?: boolean } = {}): Promise<{ url: string; body: string }> {
    if (refresh) rootInflight = null;
    if (!rootInflight) {
        const request = (async () => {
            if (!CATALOG_URL) return releaseCatalog();
            const url = resolve(CATALOG_URL);
            return { url, body: await (await get(CATALOG_URL, refresh)).text() };
        })().catch((e: unknown) => {
            if (rootInflight === request) rootInflight = null;
            throw e;
        });
        rootInflight = request;
    }
    return rootInflight;
}

/**
 * The manifest of the active planner release. It is a release object, and a 404 means that the release
 * is gone after a catalogue switch: the planner catalogue is read again, once, and the request repeats.
 */
async function releaseCatalog(): Promise<{ url: string; body: string }> {
    let url = String((await activeRelease()).device_catalog);
    let res = await fetch(url);
    if (res.status === 404) res = await fetch((url = String((await activeRelease()).device_catalog)));
    if (!res.ok) throw new Error(`${url}: ${res.status} ${res.statusText}`);
    return { url, body: await res.text() };
}

export const platform: Platform = {
    name: "web",
    caps: {
        // A browser ride library would be OPFS/IndexedDB: invisible, evictable
        // and unbackupable. Web exports one GPX and keeps no record.
        rideLibrary: false,
        // WebUSB is this tier's design, Chromium-only, which is why the desktop app
        // exists.
        deviceUsb: true,
        deviceDashboard: false,
    },

    // Chromium-only, and this tier has no other way to reach a cable — so on Safari
    // and Firefox the USB features gate on the *browser*, with their own reason and
    // their own remedy. The download-and-copy-to-the-card path is unaffected.
    //
    usbViaWebUsb: true,

    catalog: fetchCatalog,
    catalogFetch: globalThis.fetch,
    // A map is one `.obcm` again, so the web tier uses the browser's ordinary
    // download flow. This avoids Chromium's restricted directory picker and
    // makes Downloads behave like every other file from the site.
    openMapOutput: null,

    // WebUSB, loaded on demand. The import is dynamic so the transport, the protocol
    // codecs and the client land in their own chunk: a visitor who only downloads a
    // map never pays for the device stack. The session it returns is `unsupported`
    // on a browser without WebUSB rather than absent — the tier *has* the capability,
    // this browser does not, and those are different sentences for the UI to say.
    device: async () => {
        const { openWebUsbSession } = await import("../usb/session.svelte");
        return openWebUsbSession();
    },
    rides: null,

    styleEditor: null,

    // This host *is* the site, so its header links back out to the rest of it.
    siteNav: LINKS,
};
