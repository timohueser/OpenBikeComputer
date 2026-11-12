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

    catalog: fetchCatalog,
    catalogFetch: globalThis.fetch,

    styleEditor: null,

    // This host *is* the site, so its header links back out to the rest of it.
    siteNav: LINKS,
};
