/**
 * Track thumbnails for the device page's tiles: one downsampled `[lat, lon]` track per route and
 * ride on the card, cached so each object crosses the cable **once, ever**.
 *
 * Three layers, outermost first:
 *
 * - {@link DeviceThumbs} — the page-facing store. A module singleton like the dashboard (the
 *   tracks survive a tab switch), holding a reactive in-memory map for the tiles plus the
 *   persistent cache below. `fill` walks the dashboard's lists **sequentially** — every load runs
 *   through the page's `enqueue`, one small download at a time, FIFO with whatever the rider
 *   clicks — because the cable has one transfer slot and thumbnails are the least important thing
 *   on it. Tiles render immediately with an empty box and fill in as tracks land.
 * - {@link ThumbCache} — the persistence. `localStorage`, keyed by
 *   `(serial, epoch, kind, id, fingerprint)`: any change to the stored object — an edit, a
 *   re-upload, and yes, a rename, which rewrites the object and moves its CRC — moves the key
 *   and refetches. A rename refetching an unchanged track is a few KB once; a stale thumbnail
 *   surviving a content change would be a lie forever. LRU-capped at {@link THUMB_CACHE_CAP}
 *   entries; a corrupt or evicted entry is simply refetched.
 * - {@link ThumbStorage} — the five-line storage seam, so the cache's keying, LRU and round-trip
 *   behaviour are tested against a Map rather than a browser global.
 *
 * What is *stored* is points, not pixels: the tiles draw them through `fitTracks`
 * (`library.ts`), the same projection the ride library's previews use.
 */

import { SvelteMap } from "svelte/reactivity";

import { downsampleTrack, round6 } from "./library";
import { scopeKey, type RideScope } from "./rides";

/** A downsampled `[lat, lon]` polyline, ready for `fitTracks`. */
export type Thumb = ReadonlyArray<readonly [number, number]>;

export type ThumbKind = "route" | "ride";

/** The stage colors of a trip's combined preview, cycled — the field-guide palette. */
export const STAGE_COLORS = ["#3c6b39", "#cf6a2a", "#33575b", "#e3ad33", "#5f7d3d"] as const;

/** `v1` is the entry shape: bump it if {@link StoredThumb} changes, and old entries just refetch. */
const PREFIX = "obc-thumb:v1:";

/** Entries kept before the least recently used are dropped. ~5 KB each, so ~1.5 MB at the cap —
 *  well inside a browser's per-origin `localStorage` budget, with room left for the app's config. */
export const THUMB_CACHE_CAP = 300;

/** The slice of `localStorage` the cache needs — injectable, so tests hand in a Map. */
export interface ThumbStorage {
    get(key: string): string | null;
    /** May throw (quota) — the cache treats that as "evict and retry once, then give up". */
    set(key: string, value: string): void;
    remove(key: string): void;
    /** Every key currently stored; the cache filters for its own prefix. */
    keys(): string[];
}

/**
 * A route's content fingerprint: the list entry's CRC-32, or **null** where the device reported
 * none. `crc32 == 0` is a real state (a side-loaded or not-yet-fingerprinted object), not a
 * fluke — and a byte-length stand-in would let a same-length, different-content replacement
 * under a reused id keep a stale thumbnail forever. Null tells the store "no stable content
 * identity": the thumbnail lives in the session's memory map only, never the persistent cache.
 */
export function routeFingerprint(entry: { readonly crc32: number }): string | null {
    return entry.crc32 !== 0 ? `c${entry.crc32 >>> 0}` : null;
}

/** A ride's content fingerprint. The ride list carries no CRC; start time + length pin it well
 *  enough for something the device treats as immutable anyway. */
export function rideFingerprint(entry: { readonly startTime: number; readonly byteLen: number }): string {
    return `t${entry.startTime}-l${entry.byteLen}`;
}

/** The full storage key of one thumbnail. */
export function thumbKey(scope: RideScope, kind: ThumbKind, id: number, fingerprint: string): string {
    return `${PREFIX}${scopeKey(scope)}:${kind}:${id}:${fingerprint}`;
}

/** What one storage entry holds. `at` is last-use, for the LRU order. */
interface StoredThumb {
    at: number;
    track: Array<[number, number]>;
}

/** How many entries a quota failure evicts beyond the cap — breathing room, not a reset. */
const QUOTA_HEADROOM = 25;

export class ThumbCache {
    private readonly storage: ThumbStorage;
    private readonly cap: number;
    private readonly now: () => number;

    constructor(storage: ThumbStorage, cap: number = THUMB_CACHE_CAP, now: () => number = Date.now) {
        this.storage = storage;
        this.cap = cap;
        this.now = now;
    }

    /** The cached track, freshened in the LRU order — or null (absent or corrupt, either way:
     *  refetch). A corrupt entry is removed so it cannot shadow the refetched one. */
    get(key: string): Thumb | null {
        const raw = this.storage.get(key);
        if (raw === null) return null;
        const parsed = parseStored(raw);
        if (parsed === null) {
            this.storage.remove(key);
            return null;
        }
        this.write(key, { at: this.now(), track: parsed.track });
        return parsed.track;
    }

    /** Store one track, rounded to six decimals, evicting least-recently-used entries past the cap. */
    put(key: string, track: Thumb): void {
        const stored: StoredThumb = {
            at: this.now(),
            track: track.map((p) => [round6(p[0]), round6(p[1])]),
        };
        this.write(key, stored);
        this.evict(this.cap);
    }

    /** Write one entry; on a quota throw, evict well below the cap and retry once. */
    private write(key: string, stored: StoredThumb): void {
        const value = JSON.stringify(stored);
        try {
            this.storage.set(key, value);
        } catch {
            this.evict(Math.max(0, this.cap - QUOTA_HEADROOM));
            try {
                this.storage.set(key, value);
            } catch {
                // A full origin is a missing optimization, not an error: the tile refetches.
            }
        }
    }

    /** Drop thumb entries, oldest `at` first, until at most `keep` remain. */
    private evict(keep: number): void {
        const entries: Array<{ key: string; at: number }> = [];
        for (const key of this.storage.keys()) {
            if (!key.startsWith(PREFIX)) continue;
            const parsed = parseStored(this.storage.get(key) ?? "");
            if (parsed === null) {
                this.storage.remove(key);
                continue;
            }
            entries.push({ key, at: parsed.at });
        }
        if (entries.length <= keep) return;
        entries.sort((a, b) => a.at - b.at);
        for (const entry of entries.slice(0, entries.length - keep)) {
            this.storage.remove(entry.key);
        }
    }
}

/** Parse a stored entry, strictly: anything off-shape is null (and gets removed by the caller). */
function parseStored(raw: string): StoredThumb | null {
    let value: unknown;
    try {
        value = JSON.parse(raw);
    } catch {
        return null;
    }
    if (typeof value !== "object" || value === null) return null;
    const { at, track } = value as { at?: unknown; track?: unknown };
    if (typeof at !== "number" || !Array.isArray(track)) return null;
    for (const point of track) {
        if (
            !Array.isArray(point) ||
            point.length !== 2 ||
            typeof point[0] !== "number" ||
            typeof point[1] !== "number" ||
            !Number.isFinite(point[0]) ||
            !Number.isFinite(point[1])
        ) {
            return null;
        }
    }
    return { at, track: track as Array<[number, number]> };
}

// --- the page-facing store -------------------------------------------------------

/** One thumbnail the page wants: who it is, what pins its content, and how to get the points. */
export interface ThumbRequest {
    readonly kind: ThumbKind;
    readonly id: number;
    /** The content identity, or null where the device has none to offer — then the track is
     *  held for this session only and never written to the persistent cache. */
    readonly fingerprint: string | null;
    /**
     * Produce the full-resolution `[lat, lon]` track — a cable download for most objects, a read
     * of the ride library's stored preview for a ride already pulled. Downsampling is the store's
     * job, so a loader just hands over what it has.
     */
    readonly load: (signal: AbortSignal) => Promise<Thumb>;
}

/** Run one operation behind every previously queued one — the dashboard's `enqueue`, in practice. */
export type ThumbQueue = <T>(op: () => Promise<T>) => Promise<T>;

export class DeviceThumbs {
    /** What the tiles read, keyed `kind:id` within the current scope. A `SvelteMap`, so a tile
     *  re-renders the moment its track lands. */
    private readonly tracks = new SvelteMap<string, Thumb>();
    private readonly cache: ThumbCache;
    private scope: string | null = null;

    constructor(storage: ThumbStorage | null = browserStorage()) {
        this.cache = new ThumbCache(storage ?? memoryStorage());
    }

    /** The track for a tile, or null while it is still on its way. Reactive. */
    get(kind: ThumbKind, id: number): Thumb | null {
        return this.tracks.get(memKey(kind, id)) ?? null;
    }

    /** Forget the in-memory map when `(serial, epoch)` changes — ids are recycled across scopes,
     *  so a stale map would put one card's tracks on another card's tiles. The persistent cache
     *  needs no such flush: its keys carry the scope. */
    ensureScope(scope: RideScope): void {
        const key = scopeKey(scope);
        if (this.scope === key) return;
        this.scope = key;
        this.tracks.clear();
    }

    /**
     * One thumbnail: memory, then the persistent cache, then the loader — which is the only step
     * that can touch the cable, and it runs through `queue` so it never races a user action.
     *
     * The memory map is keyed by bare `kind:id` and ids are recycled across `(serial, epoch)`
     * scopes, so a completion that crossed an abort or a scope change is **dropped**, not stored:
     * device A's route 1 must never end up on device B's tile, and B's own fill must still find
     * the slot empty and fetch for itself.
     */
    async ensure(scope: RideScope, request: ThumbRequest, queue: ThumbQueue, signal: AbortSignal): Promise<Thumb> {
        this.ensureScope(scope);
        const started = this.scope;
        const key = memKey(request.kind, request.id);
        const held = this.tracks.get(key);
        if (held) return held;
        const storageKey =
            request.fingerprint === null ? null : thumbKey(scope, request.kind, request.id, request.fingerprint);
        const stored = storageKey === null ? null : this.cache.get(storageKey);
        if (stored) {
            this.tracks.set(key, stored);
            return stored;
        }
        const track = await queue(async () => {
            // Re-checked inside the queue slot: a preview click and the background fill can both
            // ask for the same object, and the second asker should find the first one's answer
            // rather than paying for a second download. Only within the same scope — under a new
            // scope the slot holds a different device's track.
            const landed = this.scope === started ? this.tracks.get(key) : null;
            if (landed) return landed;
            return downsampleTrack(await request.load(signal));
        });
        if (!signal.aborted && this.scope === started) {
            if (storageKey !== null) this.cache.put(storageKey, track);
            this.tracks.set(key, track);
        }
        return track;
    }

    /**
     * Fill every missing thumbnail, strictly one at a time and in list order. A failed load is
     * skipped — its tile keeps the empty box and the next refresh retries — unless the signal
     * aborted, which ends the walk: the page unmounted or the device is gone.
     */
    async fill(
        scope: RideScope,
        requests: readonly ThumbRequest[],
        queue: ThumbQueue,
        signal: AbortSignal,
    ): Promise<void> {
        for (const request of requests) {
            if (signal.aborted) return;
            try {
                await this.ensure(scope, request, queue, signal);
            } catch {
                if (signal.aborted) return;
            }
        }
    }
}

function memKey(kind: ThumbKind, id: number): string {
    return `${kind}:${id}`;
}

/** `localStorage` behind the seam — or null where the platform denies it (then memory-only). */
function browserStorage(): ThumbStorage | null {
    try {
        const ls = globalThis.localStorage;
        // A throwing storage (denied cookies) is caught here; a missing one (node) falls through.
        ls.getItem(`${PREFIX}probe`);
        return {
            get: (key) => ls.getItem(key),
            set: (key, value) => ls.setItem(key, value),
            remove: (key) => ls.removeItem(key),
            keys: () => {
                const out: string[] = [];
                for (let i = 0; i < ls.length; i++) {
                    const key = ls.key(i);
                    if (key !== null) out.push(key);
                }
                return out;
            },
        };
    } catch {
        return null;
    }
}

/** The fallback (and the test double's shape): a Map wearing the storage interface. */
export function memoryStorage(): ThumbStorage {
    const map = new Map<string, string>();
    return {
        get: (key) => map.get(key) ?? null,
        set: (key, value) => void map.set(key, value),
        remove: (key) => void map.delete(key),
        keys: () => [...map.keys()],
    };
}

/** The store, shared like the dashboard: thumbnails survive a tab switch. */
export const deviceThumbs = new DeviceThumbs();
