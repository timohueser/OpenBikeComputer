/**
 * The device page's model of what is on the card: routes, trips (with their stage lists), rides.
 *
 * Three rules shape everything here, all inherited from the protocol:
 *
 *   1. **One transfer at a time, enforced by throwing.** A `LIST` is an ordinary control exchange,
 *      served beside a live upload, but a trip's stage list is a `GET`. `enqueue` keeps every cable
 *      operation this page makes on one promise chain, so the page can never collide with *itself*.
 *      It can still collide with a transfer another surface started, since the rule is device-wide,
 *      and that is not an error to retry into but a state to render.
 *
 *   2. **Ids are only meaningful inside `(serial, era)`.** A card swap or a different device makes
 *      every cached entry a claim about different objects, so the store reloads when the scope
 *      changes.
 *
 *   3. **The catalog is what a client can know without downloading.** A route's distance, a ride's
 *      start time and a trip's stage list live in the payload. Only the last is fetched here,
 *      because a trip object is 56 bytes plus eight per stage and the page cannot draw a trip
 *      without it.
 *
 * A module singleton, deliberately: the lists survive a tab switch, so returning to the Device tab
 * does not re-list the card.
 */

import type { FlatStoreClient } from "../usb/client";
import { DeviceError } from "../usb/client";
import { EntryFlags, ObjectKind, type CatalogEntry } from "../usb/protocol";
import { decodeTripObject, type TripObject } from "../usb/objects";
import { scopeKey, type RideScope } from "./rides";

/** A trip as the page renders it: the catalog row plus the stage list behind it. `detail` is null
 *  only when the trip object itself could not be read — rendered as a group with no rows. */
export interface TripView extends CatalogEntry {
    readonly detail: TripObject | null;
}

export class DeviceDashboard {
    routes = $state<CatalogEntry[]>([]);
    trips = $state<TripView[]>([]);
    rides = $state<CatalogEntry[]>([]);
    loading = $state(false);
    /** A failure loading or mutating, rendered once at the top of the page. */
    error = $state<string | null>(null);
    /** A transfer owned by another surface holds the link's one slot. */
    busy = $state(false);

    private chain: Promise<unknown> = Promise.resolve();
    private loadedFor: string | null = null;

    /**
     * Run one cable operation, strictly after every previously enqueued one.
     *
     * The chain never rejects — each link settles — but the caller's promise still carries the
     * failure, so call sites decide what a failure means.
     */
    enqueue<T>(op: () => Promise<T>): Promise<T> {
        const run = this.chain.then(op, op);
        this.chain = run.then(
            () => undefined,
            () => undefined,
        );
        return run.catch((cause: unknown) => {
            if (cause instanceof DeviceError && cause.code === "busy") this.busy = true;
            throw cause;
        });
    }

    /** Forget the slot conflict — called when the rider retries, or when the header's readout
     *  shows the foreign transfer finished. */
    clearBusy(): void {
        this.busy = false;
    }

    /**
     * Load the three lists, once per `(serial, StoreId)`. A remount of the page on the same device
     * renders what is already here; a card swap or another device reloads.
     */
    async ensureLoaded(client: FlatStoreClient, scope: RideScope): Promise<void> {
        const key = scopeKey(scope);
        if (this.loadedFor === key) return;
        this.loadedFor = key;
        await this.refresh(client);
    }

    /** Re-read everything. Called after every mutation — a catalog page is metadata and the card is
     *  the authority, so re-listing beats mirroring each edit locally and drifting. */
    async refresh(client: FlatStoreClient): Promise<void> {
        this.loading = true;
        this.error = null;
        try {
            // One listing, filtered here rather than three filtered by the device. A kind filter
            // exists, but three round trips buy nothing when the whole catalog fits in a couple of
            // pages and the page wants all three kinds every time.
            const catalog = await this.enqueue(() => client.list());
            const routes = catalog.entries.filter((entry) => entry.kind === ObjectKind.Route);
            const trips = catalog.entries.filter((entry) => entry.kind === ObjectKind.Trip);
            const details: TripView[] = [];
            for (const entry of trips) {
                // Sequential on purpose — each is a `GET`, and the chain is the page's
                // serialization guarantee.
                const detail = await this.enqueue(() =>
                    client.get({ objectId: entry.objectId, revision: entry.revision }),
                )
                    .then((result) => decodeTripObject(result.bytes))
                    .catch(() => null);
                details.push({ ...entry, detail });
            }
            this.routes = routes;
            this.trips = details;
            // A ride the device is still recording has a zero length and CRC until the commit that
            // ends it, so it is listed and not offered: the page shows it as recording.
            this.rides = catalog.entries.filter((entry) => entry.kind === ObjectKind.Ride);
        } catch (cause) {
            this.error = cause instanceof Error ? cause.message : String(cause);
            // Nothing loaded is a fact worth retrying, not a scope worth remembering.
            this.loadedFor = null;
        } finally {
            this.loading = false;
        }
    }

    /**
     * Every route id that is a stage of some trip — the rows the top-level list leaves out. Full-width
     * flat-store ObjectIds, exactly as the trip payload carries them.
     */
    get stagedIds(): Set<bigint> {
        const staged = new Set<bigint>();
        for (const trip of this.trips) for (const id of trip.detail?.stages ?? []) staged.add(id);
        return staged;
    }

    /** Routes that are not inside any trip, in list order. */
    get topLevelRoutes(): CatalogEntry[] {
        const staged = this.stagedIds;
        return this.routes.filter((route) => !staged.has(route.objectId));
    }

    /**
     * A trip's stages resolved against the route list. Null marks a dangling id — a member route
     * deleted on its own, which the device tolerates and serves verbatim.
     *
     * A trip stores the catalog's complete `u64` ObjectId, so resolution stays exact even after the
     * allocation cursor has moved beyond JavaScript's safe integer range.
     */
    stagesOf(trip: TripView): Array<{ id: bigint; route: CatalogEntry | null }> {
        const byId = new Map(this.routes.map((route) => [route.objectId, route]));
        return (trip.detail?.stages ?? []).map((id) => ({ id, route: byId.get(id) ?? null }));
    }

    /** True where this entry is a ride the device is still recording, which cannot be served. */
    isRecording(entry: CatalogEntry): boolean {
        return (entry.flags & EntryFlags.Recording) !== 0;
    }

    /** Drop everything, for a disconnect: the next connect decides what to load. */
    invalidate(): void {
        this.loadedFor = null;
        this.routes = [];
        this.trips = [];
        this.rides = [];
        this.error = null;
        this.busy = false;
    }
}

export const dashboard = new DeviceDashboard();
