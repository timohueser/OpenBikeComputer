// Host-neutral catalog client. The short-cached root pins every satellite by
// length and digest; matching content-addressed satellites are memoized, while
// failed loads never alter the accepted root.

import { fetchVerified, HttpStatusError, withRetry, type DownloadOptions } from "../download";
import {
    cellIndexRef,
    parseRoot,
    region as findRegion,
    type Catalog,
    type CellIndexRef,
    type RegionEntry,
    type SkinEntry,
} from "./manifest";
import { fail } from "./parse";
import {
    assertRegionCellsIndexed,
    parseCellIndex,
    parseRegionCells,
    parseTerrainIndex,
    type CellEntry,
    type CellIndexDocument,
    type RegionCellsDocument,
    type TerrainCellEntry,
    type TerrainIndexDocument,
} from "./satellites";

export interface CatalogClientOptions {
    /** Injected by the tests and by hosts with their own transport; defaults to
     *  the global. */
    fetchImpl?: typeof fetch;
    /** Injected by the tests; defaults to `crypto.subtle`. */
    digest?: (bytes: Uint8Array) => Promise<ArrayBuffer>;
    signal?: AbortSignal;
    /** Per-object retry, forwarded to `fetchVerified`. Set to 1 by the tests that
     *  assert a refusal, so they do not sit through the backoff. */
    attempts?: number;
    sleep?: (ms: number) => Promise<void>;
}

const decoder = new TextDecoder("utf-8", { fatal: true });

function decode(bytes: Uint8Array, where: string): string {
    try {
        return decoder.decode(bytes);
    } catch {
        return fail(`${where}: the document is not valid UTF-8`);
    }
}

/**
 * A loaded cell catalog and the lazy path to its satellites.
 *
 * Construction is `CatalogClient.load(url)` rather than `new`: a client with
 * an unparsed root would be a client every caller has to check first, and §7's
 * whole-or-nothing rule is easier to keep when the invalid state is unreachable.
 */
export class CatalogClient {
    /** The parsed, self-consistent root. */
    readonly catalog: Catalog;
    /** Where the root was fetched from; every relative `url` in it resolves
     *  against this and nothing else. */
    readonly baseUrl: string;
    /** The host's byte transport, also reused by the cell download plan. */
    readonly fetchImpl: typeof fetch;

    private readonly opts: CatalogClientOptions;
    private readonly indices = new Map<string, Promise<CellIndexDocument>>();
    /** The ones that have actually arrived — a plain map rather than an "is this
     *  promise settled?" dance, which JS cannot ask without awaiting. */
    private readonly loaded = new Map<string, CellIndexDocument>();
    private readonly regionCells = new Map<string, Promise<RegionCellsDocument>>();
    private readonly previews = new Map<string, Promise<Uint8Array>>();
    private terrainIndex: Promise<TerrainIndexDocument | null> | null = null;

    private constructor(catalog: Catalog, baseUrl: string, opts: CatalogClientOptions) {
        this.catalog = catalog;
        this.baseUrl = baseUrl;
        this.opts = opts;
        this.fetchImpl = opts.fetchImpl ?? globalThis.fetch;
    }

    /**
     * Fetch and parse the root.
     *
     * `url` must be absolute: resolving a relative one is a host's job (the web
     * host has a `document.baseURI`, the desktop host does not), and a client
     * that guessed would resolve a CDN path against a Tauri asset scheme.
     *
     * The root is fetched on trust — there is no document above it to pin it —
     * which is why §9 short-caches it and content-addresses everything else.
     */
    static async load(url: string, opts: CatalogClientOptions = {}): Promise<CatalogClient> {
        let base: string;
        try {
            base = new URL(url).toString();
        } catch {
            return fail(`catalog: ${JSON.stringify(url)} is not an absolute URL`);
        }
        const doFetch = opts.fetchImpl ?? globalThis.fetch;
        // Retried like every other object in the tree. There is nothing to pin the
        // root against, but a connection dropped while it arrives is a transport
        // failure either way — and one that leaves the app with no catalog at all.
        const body = await withRetry(async () => {
            const res = await doFetch(base, { signal: opts.signal });
            if (!res.ok) throw new HttpStatusError(base, res.status, res.statusText);
            // §7 spelled out: the entire body, then one parse. Nothing here consumes
            // the response incrementally.
            return res.text();
        }, opts);
        return new CatalogClient(parseRoot(body), base, opts);
    }

    /**
     * Build a client from a root body already in hand.
     *
     * This exists for hosts that already fetched the catalog root and must not
     * fetch the same document again just to construct the client. `url` is
     * where the body actually came from — every
     * relative satellite `url` resolves against it, so handing a body with the
     * wrong origin would fetch satellites from the wrong place.
     */
    static fromBody(body: string, url: string, opts: CatalogClientOptions = {}): CatalogClient {
        let base: string;
        try {
            base = new URL(url).toString();
        } catch {
            return fail(`catalog: ${JSON.stringify(url)} is not an absolute URL`);
        }
        return new CatalogClient(parseRoot(body), base, opts);
    }

    /** Absolute URL of a `url` field in the root or a satellite. */
    resolve(url: string): string {
        return new URL(url, this.baseUrl).toString();
    }

    /** A skin's canonical PNG, admitted only after its root pin matches. */
    skinPreview(skinId: string): Promise<Uint8Array | null> {
        const skin: SkinEntry | undefined = this.catalog.skins.find((entry) => entry.id === skinId);
        if (!skin) return Promise.reject(new Error(`no skin "${skinId}" in this catalog`));
        if (!skin.preview) return Promise.resolve(null);
        const cached = this.previews.get(skinId);
        if (cached) return cached;
        const url = this.resolve(skin.preview.url);
        const inflight = fetchVerified(url, skin.preview, this.downloadOptions()).catch((e: unknown) => {
            this.previews.delete(skinId);
            throw e;
        });
        this.previews.set(skinId, inflight);
        return inflight;
    }

    /** One band's cell index, verified against the root's pin and memoised. */
    cellIndex(bandId: string): Promise<CellIndexDocument> {
        const cached = this.indices.get(bandId);
        if (cached) return cached;
        const ref = cellIndexRef(this.catalog, bandId);
        if (!ref) return Promise.reject(new Error(`no cell index for band "${bandId}"`));
        const inflight = this.loadCellIndex(ref).catch((e: unknown) => {
            // A failure pins nothing: a CDN hiccup must not make the band
            // permanently unavailable for the life of the page.
            this.indices.delete(bandId);
            throw e;
        });
        this.indices.set(bandId, inflight);
        return inflight;
    }

    /** Every band's cell index. The builder needs all of them to price anything
     *  a region does not already price, so this is the normal entry point. */
    async cellIndices(): Promise<Map<string, CellIndexDocument>> {
        const loaded = await Promise.all(this.catalog.schema.bands.map((b) => this.cellIndex(b.id)));
        return new Map(loaded.map((doc) => [doc.band, doc]));
    }

    /**
     * The single pinned terrain index (§13.1), or `null` when the catalog
     * publishes no terrain at all.
     *
     * `null` is not a failure and must not be reported as one: a terrain-less
     * catalog is complete and valid, and every map assembled from it is an
     * ordinary map whose profiles are flat. It is one document for the whole
     * store — terrain is not keyed by band — so there is one promise here rather
     * than a map of them.
     */
    terrain(): Promise<TerrainIndexDocument | null> {
        if (this.terrainIndex) return this.terrainIndex;
        const block = this.catalog.terrain;
        if (!block) return Promise.resolve(null);
        const inflight = (async () => {
            const bytes = await fetchVerified(this.resolve(block.cell_index.url), block.cell_index, this.downloadOptions());
            const doc = parseTerrainIndex(decode(bytes, "terrain index"), this.catalog, block);
            // Resolved once, here, exactly as a band cell's url is.
            const cells = doc.cells.map((c): TerrainCellEntry => ({ ...c, url: this.resolve(c.url) }));
            return { ...doc, cells, byId: new Map(cells.map((c) => [c.id, c])) };
        })().catch((e: unknown) => {
            this.terrainIndex = null;
            throw e;
        });
        this.terrainIndex = inflight;
        return inflight;
    }

    /**
     * A named region's stored cell list, verified against the root's pin.
     *
     * The list is checked against whichever band indices are already loaded
     * (§6's cross-document MUST). It is not checked against ones that are
     * not: forcing every index to load to open one region would turn a region
     * pick into four extra round trips, and the same check runs again — over the
     * full set — when a selection is resolved.
     */
    regionCellList(regionId: string): Promise<RegionCellsDocument> {
        const cached = this.regionCells.get(regionId);
        if (cached) return cached;
        const entry = findRegion(this.catalog, regionId);
        if (!entry) return Promise.reject(new Error(`no region "${regionId}" in this catalog`));
        const inflight = this.loadRegionCells(entry).catch((e: unknown) => {
            this.regionCells.delete(regionId);
            throw e;
        });
        this.regionCells.set(regionId, inflight);
        return inflight;
    }

    private async loadCellIndex(ref: CellIndexRef): Promise<CellIndexDocument> {
        const url = this.resolve(ref.url);
        const bytes = await fetchVerified(url, ref, this.downloadOptions());
        const doc = parseCellIndex(decode(bytes, `cell index (${ref.band})`), this.catalog, ref);
        // A cell's own `url` is resolved once, here, so nothing downstream holds
        // a reference that means different things depending on where it is read.
        const cells = doc.cells.map((c): CellEntry => ({ ...c, url: this.resolve(c.url) }));
        const resolved = { ...doc, cells, byId: new Map(cells.map((c) => [c.id, c])) };
        this.loaded.set(ref.band, resolved);
        return resolved;
    }

    private async loadRegionCells(entry: RegionEntry): Promise<RegionCellsDocument> {
        const url = this.resolve(entry.cells_url);
        const bytes = await fetchVerified(url, { bytes: entry.cells_bytes, sha256: entry.cells_sha256 }, this.downloadOptions());
        const doc = parseRegionCells(decode(bytes, `region cells (${entry.id})`), this.catalog, entry);
        assertRegionCellsIndexed(doc, this.loaded);
        return doc;
    }

    private downloadOptions(): DownloadOptions {
        return {
            fetchImpl: this.fetchImpl,
            digest: this.opts.digest,
            signal: this.opts.signal,
            attempts: this.opts.attempts,
            sleep: this.opts.sleep,
        };
    }
}
