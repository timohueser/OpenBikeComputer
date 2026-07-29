/**
 * Where a preset's demo map comes from, and what the card frames.
 *
 * `builder/bake-previews.sh` writes both halves into `public/preview/`: one `<preset-id>.obcm` per
 * preset in `builder/presets/`, and a `previews.json` index naming them and pinning the bbox every
 * card frames. They are static files next to the app, not part of the OBCC catalog (that document
 * is about whole baked regions, and is the bakery's — #898), and they are committed rather than
 * built by the deploy: baking wants GEOS, a 600 MB extract and minutes of CPU.
 *
 * The index is what lets a card know a demo map exists without probing for a 404, and it is why a
 * preset dropped into `builder/presets/` needs no code change here — one bake run and it is in the
 * document.
 */

/** Coverage box in microdegrees — the unit the renderer's camera speaks. */
export interface PreviewBbox {
    min_lon: number;
    min_lat: number;
    max_lon: number;
    max_lat: number;
}

/** One baked demo map. */
export interface DemoMap {
    preset_id: string;
    /** The preset version this map was baked from, for spotting a stale bake. */
    preset_version: number | null;
    /** Filename, relative to the index. */
    file: string;
    bytes: number;
    sha256: string;
}

/** `previews.json` as this app reads it. */
export interface PreviewIndex {
    schema_version: number;
    built_at: string;
    source: string;
    /** The one box every card frames — see `bake-previews.sh`, not any map's header bbox. */
    bbox: PreviewBbox;
    maps: DemoMap[];
}

/** The envelope version this consumer implements. */
export const PREVIEW_SCHEMA_VERSION = 1;

/** Where the index lives, relative to the document — so the app works mounted anywhere. */
const INDEX_URL = "./preview/previews.json";

/** A validated index plus the base its map files resolve against. */
interface Loaded {
    index: PreviewIndex;
    base: string;
}

let loading: Promise<Loaded> | null = null;

/**
 * Fetch and validate the index once per page load. Only a *fulfilled* promise is kept: a failed
 * fetch that pinned itself would make the failure permanent until a reload.
 */
export function previewIndex(): Promise<PreviewIndex> {
    return loaded().then((l) => l.index);
}

function loaded(): Promise<Loaded> {
    if (!loading) {
        const pending = fetchIndex();
        loading = pending;
        pending.catch(() => {
            if (loading === pending) loading = null;
        });
    }
    return loading;
}

async function fetchIndex(): Promise<Loaded> {
    const base = new URL(INDEX_URL, document.baseURI).toString();
    const res = await fetch(base);
    if (!res.ok) throw new Error(`${INDEX_URL}: ${res.status} ${res.statusText}`);
    return { index: parsePreviewIndex(await res.text()), base };
}

/**
 * Parse the index from a whole response body. Rejects an envelope it does not implement outright
 * rather than reading fields out of a document whose shape it cannot vouch for — the same rule
 * OBCC §7 states for the catalog, for the same reason.
 */
export function parsePreviewIndex(body: string): PreviewIndex {
    const doc = JSON.parse(body) as Partial<PreviewIndex>;
    if (doc.schema_version !== PREVIEW_SCHEMA_VERSION) {
        throw new Error(`previews.json: unsupported schema_version ${String(doc.schema_version)}`);
    }
    const b = doc.bbox;
    if (
        !b ||
        !Number.isInteger(b.min_lon) ||
        !Number.isInteger(b.min_lat) ||
        !Number.isInteger(b.max_lon) ||
        !Number.isInteger(b.max_lat) ||
        b.min_lon >= b.max_lon ||
        b.min_lat >= b.max_lat
    ) {
        throw new Error("previews.json: bbox must be four microdegree integers, min < max");
    }
    if (!Array.isArray(doc.maps) || doc.maps.some((m) => typeof m?.preset_id !== "string" || typeof m?.file !== "string")) {
        throw new Error("previews.json: maps must each carry a preset_id and a file");
    }
    return doc as PreviewIndex;
}

/**
 * The demo map bytes for a preset, or `null` where that preset has no bake yet — which is a normal
 * state, not an error: a preset added since the last bake run simply has no card art.
 */
export async function demoMapFor(presetId: string): Promise<Uint8Array | null> {
    const { index, base } = await loaded();
    const entry = index.maps.find((m) => m.preset_id === presetId);
    if (!entry) return null;
    const url = new URL(entry.file, base).toString();
    const res = await fetch(url);
    if (!res.ok) throw new Error(`${entry.file}: ${res.status} ${res.statusText}`);
    return new Uint8Array(await res.arrayBuffer());
}
