// The cell catalog client: one root, its digest-pinned satellites, and nothing
// else. Host-neutral by construction — `fetch` and the digest arrive as
// arguments, no `$host` import, no store, no DOM — because all three builder
// hosts (static web, Tauri desktop, dev server) drive the same catalog and only
// one of them has a `document.baseURI` to resolve against.
//
// The shape of the guarantee (`OBCC_Spec.md` §9): the **root** is the only
// document fetched on trust, and it is short-cached rather than
// content-addressed. Everything below it — a band's cell index, a region's cell
// list, and later every cell artifact — carries a `bytes` + `sha256` pin *in the
// root*, so a consumer that has read a valid root and a matching satellite has
// exactly the guarantee a monolithic document has: the whole consistent thing, or
// nothing. A satellite whose digest does not match is rejected and **the root is
// retained**, never patched.
//
// Satellites are therefore also safe to hold forever: they are content-addressed
// by the digest that admitted them, so this client memoises each one for its
// lifetime and a failed load pins nothing.

import { fetchVerified, type DownloadOptions } from "../download";
import {
    cellIndexRef,
    parseRoot,
    region as findRegion,
    type Catalog,
    type CellIndexRef,
    type RegionEntry,
} from "./manifest";
import { fail } from "./parse";
import {
    assertRegionCellsIndexed,
    parseCellIndex,
    parseRegionCells,
    type CellEntry,
    type CellIndexDocument,
    type RegionCellsDocument,
} from "./satellites";

export interface CatalogClientOptions {
    /** Injected by the tests and by hosts with their own transport; defaults to
     *  the global. */
    fetchImpl?: typeof fetch;
    /** Injected by the tests; defaults to `crypto.subtle`. */
    digest?: (bytes: Uint8Array) => Promise<ArrayBuffer>;
    signal?: AbortSignal;
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
        const res = await doFetch(base, { signal: opts.signal });
        if (!res.ok) throw new Error(`${base}: ${res.status} ${res.statusText}`);
        // §7 spelled out: the entire body, then one parse. Nothing here consumes
        // the response incrementally.
        return new CatalogClient(parseRoot(await res.text()), base, opts);
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
        return { fetchImpl: this.fetchImpl, digest: this.opts.digest, signal: this.opts.signal };
    }
}
