import type { RequestParameters } from "maplibre-gl";

/** The planner catalogue of specs/planner-release.md. The site reads it at page load, so a release needs no site build. */
export const PLANNER_CATALOG = "https://maps.openbikecomputer.com/planner/catalog.json";

const UNAVAILABLE = "The map catalogue is unavailable. Reload the page later.";

/** The `active` entry of the planner catalogue. A catalogue format that this code does not know throws a message for the rider. */
export async function activeRelease(): Promise<Record<string, unknown>> {
    // Revalidate, so a page never keeps a browser copy beyond the catalogue's own cache lifetime.
    const response = await fetch(PLANNER_CATALOG, { cache: "no-cache" }).catch(() => { throw new Error(UNAVAILABLE); });
    if (!response.ok) throw new Error(UNAVAILABLE);
    const catalog = await response.json();
    if (catalog?.format !== 1) throw new Error("The map catalogue has a format that this page does not know. Reload the page later.");
    if (!catalog.active || typeof catalog.active !== "object") throw new Error("The map catalogue has no active release.");
    return catalog.active;
}

// The release of this page, which only the page sets: a worker never reloads.
let loaded: string | undefined;
let saved: () => Promise<boolean> = async () => true;
let check: Promise<void> | undefined;

/** Sets the release of the page. A release object that fails can then reload the page on a new active release. */
export function pageRelease(id: string | undefined): void {
    loaded = id;
}

/** `wait` resolves true when the page can reload without losing work. */
export function beforeReload(wait: () => Promise<boolean>): void {
    saved = wait;
}

// A removed release answers 404, or no answer that the page can read: its API path has no CORS headers.
function recheck(): void {
    if (!loaded) return;
    check ??= activeRelease().then(async (active) => { if (active.id !== loaded && await saved()) location.reload(); }, () => {})
        .finally(() => { check = undefined; });
}

/**
 * `fetch` for an object of the page's release. Release objects are immutable, so a failure can mean that a catalogue
 * switch removed the release: the catalogue is read again, once, and the page loads again on a new active release.
 */
export async function releaseFetch(url: string, init?: RequestInit): Promise<Response> {
    try {
        const response = await fetch(url, init);
        if (response.status === 404) recheck();
        return response;
    } catch (error) {
        if (!init?.signal?.aborted) recheck();
        throw error;
    }
}

const SCHEME = "release://";

/** `url` on the release protocol when the page has a release, so MapLibre loads it through `releaseFetch`. */
export const releaseUrl = (url: string) => (loaded && url.startsWith("https://") ? SCHEME + url.slice(8) : url);

/** MapLibre treats a tile 404 as an empty tile, so its release objects load here; a TileJSON keeps its tiles on this protocol. */
export async function releaseProtocol({ url, type }: RequestParameters, abort: AbortController) {
    const response = await releaseFetch("https://" + url.slice(SCHEME.length), { signal: abort.signal });
    if (!response.ok) throw new Error(`${url}: HTTP ${response.status}`);
    if (type !== "json") return { data: await response.arrayBuffer() };
    const json = await response.json();
    return { data: Array.isArray(json.tiles) ? { ...json, tiles: json.tiles.map(releaseUrl) } : json };
}
