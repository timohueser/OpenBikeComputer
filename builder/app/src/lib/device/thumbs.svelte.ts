/**
 * Track thumbnails for the device page's tiles: one downsampled `[lat, lon]` track per route and
 * ride on the card. The hosted web app holds them for the current page session; the installed
 * desktop app keeps a bounded local cache so reconnecting a device does not download every object
 * again after each restart.
 *
 * The module singleton survives tab switches inside the app. Persistent storage is selected from
 * the build-time platform: desktop gets `localStorage`, while web and dev get no persistent store.
 * Cache keys carry the device scope and an object fingerprint, so changed routes are never shown
 * with an old track. Entries without a stable fingerprint remain session-only on every host.
 *
 * `fill` walks the dashboard's lists sequentially. Every load runs through the page's `enqueue`,
 * FIFO with whatever the rider clicks, because the cable has one transfer slot and thumbnails are
 * the least important work on it. Tiles render immediately with an empty box and fill in as tracks
 * land. What is held is points, not pixels; the tiles draw them through `fitTracks` (`library.ts`).
 */

import { SvelteMap } from "svelte/reactivity";

import { platform, type PlatformName } from "../platform";
import { downsampleTrack, round6 } from "./library";
import { scopeKey, type RideScope } from "./rides";

/** A downsampled `[lat, lon]` polyline, ready for `fitTracks`. */
export type Thumb = ReadonlyArray<readonly [number, number]>;

export type ThumbKind = "route" | "ride";

/** The stage colors of a trip's combined preview, cycled — the field-guide palette. */
export const STAGE_COLORS = ["#3c6b39", "#cf6a2a", "#33575b", "#e3ad33", "#5f7d3d"] as const;

const PREFIX = "obc-thumb:v1:";

/** About 1.5 MiB at the usual track size; older entries are removed least-recently-used first. */
export const THUMB_CACHE_CAP = 300;

/** The small persistent-storage seam, injectable for tests. */
export interface ThumbStorage {
    get(key: string): string | null;
    set(key: string, value: string): void;
    remove(key: string): void;
    keys(): string[];
}

/** A route CRC is its stable content identity. Zero means the device did not provide one. */
export function routeFingerprint(entry: { readonly crc32: number }): string | null {
    return entry.crc32 !== 0 ? `c${entry.crc32 >>> 0}` : null;
}

/** Rides are immutable on the device; start time plus byte length identifies their contents. */
export function rideFingerprint(entry: { readonly startTime: number; readonly byteLen: number }): string {
    return `t${entry.startTime}-l${entry.byteLen}`;
}

export function thumbKey(scope: RideScope, kind: ThumbKind, id: number, fingerprint: string): string {
    return `${PREFIX}${scopeKey(scope)}:${kind}:${id}:${fingerprint}`;
}

interface StoredThumb {
    at: number;
    track: Array<[number, number]>;
}

const QUOTA_HEADROOM = 25;

/** Bounded, defensive LRU storage. A corrupt or unavailable entry is simply downloaded again. */
export class ThumbCache {
    constructor(
        private readonly storage: ThumbStorage,
        private readonly cap: number = THUMB_CACHE_CAP,
        private readonly now: () => number = Date.now,
    ) {}

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

    put(key: string, track: Thumb): void {
        this.write(key, {
            at: this.now(),
            track: track.map((point) => [round6(point[0]), round6(point[1])]),
        });
        this.evict(this.cap);
    }

    clear(): number {
        let removed = 0;
        for (const key of this.storage.keys()) {
            if (!key.startsWith(PREFIX)) continue;
            this.storage.remove(key);
            removed += 1;
        }
        return removed;
    }

    private write(key: string, stored: StoredThumb): void {
        const value = JSON.stringify(stored);
        try {
            this.storage.set(key, value);
        } catch {
            this.evict(Math.max(0, this.cap - QUOTA_HEADROOM));
            try {
                this.storage.set(key, value);
            } catch {
                // A full or denied store removes the optimization, not the feature.
            }
        }
    }

    private evict(keep: number): void {
        const entries: Array<{ key: string; at: number }> = [];
        for (const key of this.storage.keys()) {
            if (!key.startsWith(PREFIX)) continue;
            const parsed = parseStored(this.storage.get(key) ?? "");
            if (parsed === null) {
                this.storage.remove(key);
            } else {
                entries.push({ key, at: parsed.at });
            }
        }
        if (entries.length <= keep) return;
        entries.sort((a, b) => a.at - b.at);
        for (const entry of entries.slice(0, entries.length - keep)) this.storage.remove(entry.key);
    }
}

function parseStored(raw: string): StoredThumb | null {
    let value: unknown;
    try {
        value = JSON.parse(raw);
    } catch {
        return null;
    }
    if (typeof value !== "object" || value === null) return null;
    const { at, track } = value as { at?: unknown; track?: unknown };
    if (typeof at !== "number" || !Number.isFinite(at) || !Array.isArray(track)) return null;
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

/** One thumbnail the page wants and how to fetch its full-resolution points. */
export interface ThumbRequest {
    readonly kind: ThumbKind;
    readonly id: number;
    /** Stable content identity. Without one, persistence is intentionally skipped. */
    readonly fingerprint: string | null;
    /** A cable download for most objects, or the ride library's held preview for a pulled ride. */
    readonly load: (signal: AbortSignal) => Promise<Thumb>;
}

/** Run one operation behind every previously queued one — the dashboard's `enqueue`, in practice. */
export type ThumbQueue = <T>(op: () => Promise<T>) => Promise<T>;

export class DeviceThumbs {
    /** What the tiles read, keyed `kind:id` within the current scope. */
    private readonly tracks = new SvelteMap<string, { fingerprint: string | null; track: Thumb }>();
    private readonly cache: ThumbCache | null;
    private scope: string | null = null;

    constructor(storage: ThumbStorage | null = null) {
        this.cache = storage ? new ThumbCache(storage) : null;
    }

    /** The track for a tile, or null while it is still on its way. Reactive. */
    get(kind: ThumbKind, id: number): Thumb | null {
        return this.tracks.get(memKey(kind, id))?.track ?? null;
    }

    /** Forget the map when `(serial, epoch)` changes; ids are recycled across device scopes. */
    ensureScope(scope: RideScope): void {
        const key = scopeKey(scope);
        if (this.scope === key) return;
        this.scope = key;
        this.tracks.clear();
    }

    /**
     * One thumbnail: memory, then the loader. The load runs through `queue`, so it cannot race a
     * user action on the single transfer slot.
     *
     * A completion that crossed an abort or scope change is dropped: device A's route 1 must never
     * land on device B's route 1 tile.
     */
    async ensure(scope: RideScope, request: ThumbRequest, queue: ThumbQueue, signal: AbortSignal): Promise<Thumb> {
        this.ensureScope(scope);
        const started = this.scope;
        const key = memKey(request.kind, request.id);
        const held = this.tracks.get(key);
        if (held?.fingerprint === request.fingerprint) return held.track;
        const storageKey =
            request.fingerprint === null ? null : thumbKey(scope, request.kind, request.id, request.fingerprint);
        const stored = storageKey === null ? null : (this.cache?.get(storageKey) ?? null);
        if (stored) {
            this.tracks.set(key, { fingerprint: request.fingerprint, track: stored });
            return stored;
        }
        const track = await queue(async () => {
            // A preview click and the background fill can both ask for the same object while one is
            // queued. Recheck after entering the slot so the second request reuses the first.
            const landed = this.scope === started ? this.tracks.get(key) : null;
            if (landed?.fingerprint === request.fingerprint) return landed.track;
            return downsampleTrack(await request.load(signal));
        });
        if (!signal.aborted && this.scope === started) {
            if (storageKey !== null) this.cache?.put(storageKey, track);
            this.tracks.set(key, { fingerprint: request.fingerprint, track });
        }
        return track;
    }

    /** Delete durable previews. Current on-screen previews stay in memory until the app closes. */
    clearPersistent(): number {
        return this.cache?.clear() ?? 0;
    }

    /** Fill every missing thumbnail one at a time. A failed load is skipped; abort ends the walk. */
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

/** `localStorage` behind the seam, when the host permits it. */
function browserStorage(): ThumbStorage | null {
    try {
        const storage = globalThis.localStorage;
        storage.getItem(`${PREFIX}probe`);
        return {
            get: (key) => storage.getItem(key),
            set: (key, value) => storage.setItem(key, value),
            remove: (key) => storage.removeItem(key),
            keys: () => {
                const keys: string[] = [];
                for (let i = 0; i < storage.length; i++) {
                    const key = storage.key(i);
                    if (key !== null) keys.push(key);
                }
                return keys;
            },
        };
    } catch {
        return null;
    }
}

/** A Map wearing the storage interface, used by tests and harmless fallbacks. */
export function memoryStorage(): ThumbStorage {
    const map = new Map<string, string>();
    return {
        get: (key) => map.get(key) ?? null,
        set: (key, value) => void map.set(key, value),
        remove: (key) => void map.delete(key),
        keys: () => [...map.keys()],
    };
}

/** Pure policy seam: only the installed desktop build receives durable thumbnail storage. */
export function thumbnailStorageFor(host: PlatformName, storage: ThumbStorage | null): ThumbStorage | null {
    return host === "desktop" ? storage : null;
}

/** Remove old web entries while preserving the desktop cache. */
function purgeWebThumbs(storage: ThumbStorage | null): void {
    if (platform.name === "desktop" || storage === null) return;
    for (const key of storage.keys()) {
        if (key.startsWith(PREFIX)) storage.remove(key);
    }
}

const hostStorage = browserStorage();
purgeWebThumbs(hostStorage);

/** Shared session store; durable backing exists in the installed desktop build only. */
export const deviceThumbs = new DeviceThumbs(thumbnailStorageFor(platform.name, hostStorage));
