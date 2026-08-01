// The static hosted host: files on a CDN and nothing else — no backend at all,
// which is the whole point of the hosted tier (#894). Everything it serves is
// either a baked artifact or something wasm computes in the tab.
//
// The host fetches `catalog.json`, the OBCC manifest the bakery publishes. It
//     may live on the same origin or on the object storage the artifacts do,
//     hence its own override: the artifact `url`s inside it are absolute.
//
// Both are relative to the document by default, so the site works mounted at
// "/" or under a sub-path without a rebuild.

import { LINKS } from "../constants";
import type { Platform } from "./types";

// `||`, not `??`: a deployment that has no catalog to point at yet (the site deploy
// passes the repository variable straight through, and an unset variable arrives as
// an empty string) must fall back to the default rather than treat "" as a URL —
// which resolves to the page itself and reports a JSON parse error for an HTML body.
const DATA_BASE: string = import.meta.env.VITE_DATA_BASE || "./data";
const CATALOG_URL: string = import.meta.env.VITE_CATALOG_URL || `${DATA_BASE}/catalog.json`;

// Every seam this host declares is implemented now — C1 (#900) filled in the
// three data calls and C3 (#902) the device one — so the `pending()` helper the
// other two hosts still use has nothing left to name here.

/** Absolute URL of a static document, so a relative default resolves against
 *  the page rather than the module. */
function resolve(url: string): string {
    return new URL(url, document.baseURI).toString();
}

async function get(url: string): Promise<Response> {
    const res = await fetch(resolve(url));
    if (!res.ok) throw new Error(`${url}: ${res.status} ${res.statusText}`);
    return res;
}

/**
 * Static documents are immutable for the life of a page load, so each request
 * is made once. Only a *fulfilled* promise is kept: a failed fetch
 * that pinned itself would make the failure permanent until a reload.
 */
function once<T>(load: () => Promise<T>): () => Promise<T> {
    let inflight: Promise<T> | null = null;
    return () => {
        inflight ??= load().catch((e: unknown) => {
            inflight = null;
            throw e;
        });
        return inflight;
    };
}

/**
 * The root document as fetched. Validation belongs to `CatalogClient`, not the
 * host seam, so there is one request surface and one parser.
 */
let rootInflight: Promise<{ url: string; body: string }> | null = null;

function fetchCatalog(): Promise<{ url: string; body: string }> {
    rootInflight ??= (async () => {
        const url = resolve(CATALOG_URL);
        return { url, body: await (await get(CATALOG_URL)).text() };
    })().catch((e: unknown) => {
        rootInflight = null;
        throw e;
    });
    return rootInflight;
}

const catalogOnce = once(fetchCatalog);

export const platform: Platform = {
    name: "web",
    caps: {
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

    catalog: catalogOnce,
    catalogFetch: globalThis.fetch,
    openMapOutput: null,

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

    styleEditor: null,

    // This host *is* the site, so its header links back out to the rest of it.
    siteNav: LINKS,
};
