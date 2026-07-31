// The static hosted host: files on a CDN and nothing else — no backend at all,
// which is the whole point of the hosted tier (#894). Everything it serves is
// either a baked artifact or something wasm computes in the tab.
//
// Two static documents back this host, both fetched once per session:
//
//   * `regions.json` — the Geofabrik download index, trimmed and simplified,
//     byte-for-byte what the dev server's `/api/regions` returns. It is site
//     data, so it sits next to the app (`builder/server/static_data.py`
//     writes it; C6 #905 wires that into the deploy).
//   * `catalog.json` — the OBCC manifest the bakery publishes (B1 #898). It
//     may live on the same origin or on the object storage the artifacts do,
//     hence its own override: the artifact `url`s inside it are absolute.
//
// Both are relative to the document by default, so the site works mounted at
// "/" or under a sub-path without a rebuild.

import { parseCatalog, type Catalog } from "../catalog/manifest";
import { catalogPresets } from "../catalog/presets";
import { LINKS } from "../constants";
import type { LoadStyleEditor, Platform, RegionFeature } from "./types";

// `||`, not `??`: a deployment that has no catalog to point at yet (the site deploy
// passes the repository variable straight through, and an unset variable arrives as
// an empty string) must fall back to the default rather than treat "" as a URL —
// which resolves to the page itself and reports a JSON parse error for an HTML body.
const DATA_BASE: string = import.meta.env.VITE_DATA_BASE || "./data";
const REGIONS_URL: string = `${DATA_BASE}/regions.json`;
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
 * Both documents are immutable for the life of a page load and have more than
 * one caller (the picker draws the regions, the catalog store joins them), so
 * the request is made once. Only a *fulfilled* promise is kept: a failed fetch
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

async function fetchRegions(): Promise<RegionFeature[]> {
    const body = (await (await get(REGIONS_URL)).json()) as { features?: RegionFeature[] };
    if (!Array.isArray(body.features)) throw new Error(`${REGIONS_URL}: no features array`);
    return body.features;
}

/**
 * The root document as fetched, shared by `catalog()` and `catalogRoot` so the
 * two views of it cannot come from different responses — envelope detection
 * (#1038) peeks at this body and the flow it picks then parses the same body.
 *
 * Its memo has one subtlety the generic `once` cannot express: a fetch that
 * *succeeded* but delivered a body the parser then refuses must also be
 * dropped, or one bad response would be pinned until a reload and the catalog
 * store's retry would re-parse the same bytes forever. `fetchCatalog` below
 * owns that drop, because only it knows the parse failed.
 */
let rootInflight: Promise<{ url: string; body: string }> | null = null;

function rootOnce(): Promise<{ url: string; body: string }> {
    rootInflight ??= (async () => {
        const url = resolve(CATALOG_URL);
        return { url, body: await (await get(CATALOG_URL)).text() };
    })().catch((e: unknown) => {
        rootInflight = null;
        throw e;
    });
    return rootInflight;
}

/**
 * OBCC §7: read the entire body, then parse it as one document. The whole body
 * before `parseCatalog` is that rule spelled out — a truncated manifest cannot
 * survive a whole-document parse, and nothing here consumes the response
 * incrementally.
 *
 * The one thing this does to the document it just validated is resolve each
 * preview reference into an absolute URL. §2 says a preview resolves against
 * the same base as an artifact's `url`, and the manifest's own location is that
 * base — a fact only this module has. Doing it here means everything
 * downstream, including the copy the store caches, holds one already-resolved
 * document instead of a relative reference that means different things
 * depending on where it is read.
 */
async function fetchCatalog(): Promise<Catalog> {
    const { url: base, body } = await rootOnce();
    let catalog: Catalog;
    try {
        catalog = parseCatalog(body);
    } catch (e) {
        // The body is not a catalog this flow accepts: un-pin it, so the next
        // call fetches fresh instead of re-refusing the same bytes.
        rootInflight = null;
        throw e;
    }
    return {
        ...catalog,
        presets: catalog.presets.map((p) =>
            p.preview ? { ...p, preview: new URL(p.preview, base).toString() } : p,
        ),
    };
}

const regionsOnce = once(fetchRegions);
const catalogOnce = once(fetchCatalog);

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

    regions: regionsOnce,
    // The hosted tier's preset list is the manifest's own `presets[]` — there
    // is no packer here, so a preset names a baked artifact rather than a
    // recipe, and it arrives on the same fetch the catalog does.
    presets: () => catalogOnce().then(catalogPresets),
    catalog: catalogOnce,
    catalogRoot: rootOnce,

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

    // This host *is* the site, so its header links back out to the rest of it.
    siteNav: LINKS,
};

export const loadStyleEditor: LoadStyleEditor | null = null;
