import { PMTiles } from 'pmtiles';

export type Bounds = [number, number, number, number];
/** The body of tile z/x/y, or undefined for an absent tile. */
export type TileGetter = (z: number, x: number, y: number, signal: AbortSignal) => Promise<ArrayBuffer | undefined>;

/**
 * The signal of one request. A request fails after the same limit as the map's terrain tiles, so its
 * load leaves the cache and runs again on the next read instead of waiting forever.
 */
export const requestSignal = () => AbortSignal.timeout(20_000);

/** Tile z/x/y of a `{z}/{x}/{y}` URL template. A tile service answers 204 for an absent tile. */
export async function fetchTile(template: string, z: number, x: number, y: number, signal: AbortSignal): Promise<ArrayBuffer | undefined> {
    const response = await fetch(template.replace('{z}', String(z)).replace('{x}', String(x)).replace('{y}', String(y)), { signal });
    if (response.status === 204) return undefined;
    if (!response.ok) throw new Error(`Tile ${z}/${x}/${y} answered ${response.status}.`);
    return response.arrayBuffer();
}

/** Decoded tiles; an absent tile is null. */
export interface Tiles<T> {
    /** An abort stops this wait only: the load runs on for the cache and for other callers. */
    tile(z: number, x: number, y: number, signal?: AbortSignal): Promise<T | null>;
    /** The tile if it has arrived: null for an absent tile, undefined before it arrives. */
    loaded(z: number, x: number, y: number): T | null | undefined;
    /** Changes when a tile arrives or leaves. */
    readonly version: number;
    /** Decoded bytes in the cache. */
    readonly held: number;
}

function waiting<T>(load: Promise<T>, signal?: AbortSignal): Promise<T> {
    if (!signal) return load;
    return new Promise((resolve, reject) => {
        const abort = () => reject(signal.reason);
        if (signal.aborted) return abort();
        signal.addEventListener('abort', abort, { once: true });
        load.then(resolve, reject).finally(() => signal.removeEventListener('abort', abort));
    });
}

/**
 * Each tile is requested and decoded once while it stays cached. The cache keeps at most `budget`
 * decoded bytes; the least recently used tiles leave first. A failed load is not kept.
 */
export function decodedTiles<T>(get: TileGetter, decode: (body: Uint8Array<ArrayBuffer>, z: number, x: number, y: number) => T | Promise<T>, budget: number, bytes: (tile: T) => number): Tiles<T> {
    type Entry = { load: Promise<T | null>; tile?: T | null; bytes: number };
    const entries = new Map<string, Entry>();
    let held = 0, version = 0;
    const touch = (key: string, entry: Entry) => { entries.delete(key); entries.set(key, entry); };
    const arrive = (key: string, entry: Entry, tile: T | null) => {
        if (entries.get(key) !== entry) return;
        entry.tile = tile;
        entry.bytes = tile === null ? 0 : bytes(tile);
        held += entry.bytes;
        version++;
        for (const [old, item] of entries) {
            if (held <= budget) break;
            if (item === entry || item.tile === undefined) continue;
            entries.delete(old);
            held -= item.bytes;
        }
    };
    return {
        tile(z, x, y, signal) {
            const key = `${z}/${x}/${y}`;
            let entry = entries.get(key);
            if (entry) touch(key, entry);
            else {
                const signal = requestSignal();
                const created: Entry = entry = { load: waiting(get(z, x, y, signal), signal).then(body => body ? decode(new Uint8Array(body), z, x, y) : null), bytes: 0 };
                entries.set(key, created);
                created.load.then(tile => arrive(key, created, tile), () => { if (entries.get(key) === created) entries.delete(key); });
            }
            return waiting(entry.load, signal);
        },
        loaded(z, x, y) {
            const key = `${z}/${x}/${y}`, entry = entries.get(key);
            if (entry?.tile === undefined) return undefined;
            touch(key, entry);
            return entry.tile;
        },
        get version() { return version; },
        get held() { return held; },
    };
}

/** The raw tiles of an archive and its header. */
export interface Source { metadata: Record<string, unknown>; minZoom: number; maxZoom: number; bounds: Bounds; get: TileGetter }

/**
 * A hosted region serves TileJSON (with the archive metadata at the top level) and tiles from the
 * tile service; a local region reads the PMTiles archive.
 */
export async function openSource(url: string): Promise<Source> {
    const signal = requestSignal();
    if (new URL(url).pathname.endsWith('.json')) {
        const response = await fetch(url, { signal });
        if (!response.ok) throw new Error(`TileJSON answered ${response.status}.`);
        const json = await response.json();
        const template: string = json.tiles[0];
        return { metadata: json, minZoom: json.minzoom, maxZoom: json.maxzoom, bounds: json.bounds, get: (z, x, y, signal) => fetchTile(template, z, x, y, signal) };
    }
    const tiles = new PMTiles(url);
    // PMTiles reads its header without a signal.
    const [header, metadata] = await waiting(Promise.all([tiles.getHeader(), tiles.getMetadata() as Promise<Record<string, unknown>>]), signal);
    return {
        metadata, minZoom: header.minZoom, maxZoom: header.maxZoom, bounds: [header.minLon, header.minLat, header.maxLon, header.maxLat],
        get: async (z, x, y, signal) => (await tiles.getZxy(z, x, y, signal))?.data,
    };
}

/** How a data layer reads its archive. */
export interface Decoding<M, T> {
    meta(json: Record<string, unknown>): M;
    decode(body: Uint8Array<ArrayBuffer>, meta: M, z: number, x: number, y: number): T | Promise<T>;
    bytes(tile: T): number;
    budgetBytes: number;
}

export interface TileArchive<M, T> extends Tiles<T> { meta: M; minZoom: number; maxZoom: number; bounds: Bounds }

export function archiveOf<M, T>(source: Source, decoding: Decoding<M, T>): TileArchive<M, T> {
    const meta = decoding.meta(source.metadata);
    const tiles = decodedTiles(source.get, (body, z, x, y) => decoding.decode(body, meta, z, x, y), decoding.budgetBytes, decoding.bytes);
    // Assigned onto the tiles, which keeps their getters live.
    return Object.assign(tiles, { meta, minZoom: source.minZoom, maxZoom: source.maxZoom, bounds: source.bounds });
}

const opened = new Map<string, Promise<TileArchive<unknown, unknown>>>();

/** One archive per URL, so the layers of one archive share its tiles; a failed open is retried on the next call. */
export function openArchive<M, T>(url: string, decoding: Decoding<M, T>): Promise<TileArchive<M, T>> {
    let entry = opened.get(url) as Promise<TileArchive<M, T>> | undefined;
    if (!entry) {
        entry = openSource(url).then(source => archiveOf(source, decoding));
        opened.set(url, entry);
        entry.catch(() => opened.delete(url));
    }
    return entry;
}

/**
 * Loads each tile under the points of a line once and hands it to `read` with the indexes of its
 * points. A long route crosses tens of tiles, so a few load at a time and the map keeps the network.
 */
export async function eachTile<T>(spots: readonly { x: number; y: number }[], load: (x: number, y: number) => Promise<T>, read: (tile: T, indexes: number[]) => void, signal?: AbortSignal): Promise<void> {
    const groups = new Map<string, number[]>();
    spots.forEach(({ x, y }, i) => {
        const key = `${x}/${y}`;
        groups.get(key)?.push(i) ?? groups.set(key, [i]);
    });
    const queue = [...groups.values()];
    let next = 0;
    await Promise.all(Array.from({ length: 4 }, async () => {
        while (next < queue.length) {
            const indexes = queue[next++], { x, y } = spots[indexes[0]];
            const tile = await load(x, y);
            signal?.throwIfAborted();
            read(tile, indexes);
        }
    }));
}
