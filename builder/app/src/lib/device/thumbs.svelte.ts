/**
 * Track thumbnails for the device page's tiles: one downsampled `[lat, lon]` track per route and
 * ride on the card, held for the current app session.
 *
 * The module singleton survives tab switches inside the app, so an object crosses the cable at
 * most once while the page is open. It deliberately does not use browser persistence: a reload
 * forgets the device's coordinate tracks and fetches them again when its page is opened.
 *
 * `fill` walks the dashboard's lists sequentially. Every load runs through the page's `enqueue`,
 * FIFO with whatever the rider clicks, because the cable has one transfer slot and thumbnails are
 * the least important work on it. Tiles render immediately with an empty box and fill in as tracks
 * land. What is held is points, not pixels; the tiles draw them through `fitTracks` (`library.ts`).
 */

import { SvelteMap } from "svelte/reactivity";

import { downsampleTrack } from "./library";
import { scopeKey, type RideScope } from "./rides";

/** A downsampled `[lat, lon]` polyline, ready for `fitTracks`. */
export type Thumb = ReadonlyArray<readonly [number, number]>;

export type ThumbKind = "route" | "ride";

/** The stage colors of a trip's combined preview, cycled — the field-guide palette. */
export const STAGE_COLORS = ["#3c6b39", "#cf6a2a", "#33575b", "#e3ad33", "#5f7d3d"] as const;

/** One thumbnail the page wants and how to fetch its full-resolution points. */
export interface ThumbRequest {
    readonly kind: ThumbKind;
    readonly id: number;
    /** A cable download for most objects, or the ride library's held preview for a pulled ride. */
    readonly load: (signal: AbortSignal) => Promise<Thumb>;
}

/** Run one operation behind every previously queued one — the dashboard's `enqueue`, in practice. */
export type ThumbQueue = <T>(op: () => Promise<T>) => Promise<T>;

export class DeviceThumbs {
    /** What the tiles read, keyed `kind:id` within the current scope. */
    private readonly tracks = new SvelteMap<string, Thumb>();
    private scope: string | null = null;

    /** The track for a tile, or null while it is still on its way. Reactive. */
    get(kind: ThumbKind, id: number): Thumb | null {
        return this.tracks.get(memKey(kind, id)) ?? null;
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
        if (held) return held;
        const track = await queue(async () => {
            // A preview click and the background fill can both ask for the same object while one is
            // queued. Recheck after entering the slot so the second request reuses the first.
            const landed = this.scope === started ? this.tracks.get(key) : null;
            if (landed) return landed;
            return downsampleTrack(await request.load(signal));
        });
        if (!signal.aborted && this.scope === started) this.tracks.set(key, track);
        return track;
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

/** Remove coordinate tracks written by releases before thumbnails became session-only. */
function purgeLegacyThumbs(): void {
    try {
        const storage = globalThis.localStorage;
        for (let i = storage.length - 1; i >= 0; i--) {
            const key = storage.key(i);
            if (key?.startsWith("obc-thumb:v1:")) storage.removeItem(key);
        }
    } catch {
        // Denied or unavailable storage also means there is nothing this page can clean up.
    }
}

purgeLegacyThumbs();

/** The session store, shared like the dashboard so thumbnails survive an in-app tab switch. */
export const deviceThumbs = new DeviceThumbs();
