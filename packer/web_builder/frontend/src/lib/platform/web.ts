// The static hosted host: files on a CDN and nothing else — no backend at all,
// which is the whole point of the hosted tier (#894). Everything it serves is
// either a baked artifact or something wasm computes in the tab.
//
// Two static documents back this host, both fetched once per session:
//
//   * `regions.json` — the Geofabrik download index, trimmed and simplified,
//     byte-for-byte what the dev server's `/api/regions` returns. It is site
//     data, so it sits next to the app (`packer/web_builder/static_data.py`
//     writes it; C6 #905 wires that into the deploy).
//   * `catalog.json` — the OBCC manifest the bakery publishes (B1 #898). It
//     may live on the same origin or on the object storage the artifacts do,
//     hence its own override: the artifact `url`s inside it are absolute.
//
// Both are relative to the document by default, so the site works mounted at
// "/" or under a sub-path without a rebuild.

import { parseCatalog, type Catalog } from "../catalog/manifest";
import { catalogPresets } from "../catalog/presets";
import {
    PlatformNotImplemented,
    type LoadStyleEditor,
    type Platform,
    type RegionFeature,
} from "./types";

const DATA_BASE: string = import.meta.env.VITE_DATA_BASE ?? "./data";
const REGIONS_URL: string = `${DATA_BASE}/regions.json`;
const CATALOG_URL: string = import.meta.env.VITE_CATALOG_URL ?? `${DATA_BASE}/catalog.json`;

// `async` so an unimplemented seam *rejects* rather than throwing past the
// caller's `.catch()` — every method here is declared to return a promise, and
// a seam that isn't written yet must not also break that contract.
async function pending(member: string, owner: string): Promise<never> {
    throw new PlatformNotImplemented("web", member, owner);
}

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
 * OBCC §7: read the entire body, then parse it as one document. `res.text()`
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
    const base = resolve(CATALOG_URL);
    const catalog = parseCatalog(await (await get(CATALOG_URL)).text());
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

    regions: regionsOnce,
    // The hosted tier's preset list is the manifest's own `presets[]` — there
    // is no packer here, so a preset names a baked artifact rather than a
    // recipe, and it arrives on the same fetch the catalog does.
    presets: () => catalogOnce().then(catalogPresets),
    catalog: catalogOnce,

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
