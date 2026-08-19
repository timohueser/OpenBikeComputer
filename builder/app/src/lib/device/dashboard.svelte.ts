/**
 * The device page's model of what is on the card: routes, trips (with their stage lists), rides.
 *
 * Three rules shape everything here, all inherited from the protocol:
 *
 *   1. **One transfer at a time, enforced by throwing** (§1). A `LIST` is not a transfer any more —
 *      it is an ordinary control exchange, served beside a live upload — but a trip's stage list is
 *      a `GET`, and a `GET` is. `enqueue` keeps every cable operation this page makes on one promise
 *      chain, so the page can never collide with *itself*. It can still collide with a transfer
 *      another surface started — a map send from the builder tab, or a phone over BLE, since §1's
 *      rule is device-wide — and that is not an error to retry into but a state to render:
 *      {@link DeviceDashboard.busy} says so, and the page offers a Retry.
 *
 *   2. **Ids are only meaningful inside `(serial, era)`.** A card swap or a different device makes
 *      every cached entry a claim about different objects, so the store remembers which scope it
 *      loaded for and reloads when the scope changes.
 *
 *   3. **The catalog is what a client can know without downloading.** §3.3's entry is id, revision,
 *      payload length, payload CRC, kind, flags and a display name — and that is the whole of it.
 *      A route's distance, a ride's start time and a trip's stage list live in the payload. Only the
 *      last of those is fetched here, because a trip object is 56 bytes plus two per stage and the
 *      page cannot draw a trip without it; a route's or a ride's figures would cost a whole object
 *      each and are shown after the rider asks for one.
 *
 * A module singleton, deliberately: the lists survive a tab switch (the component unmounts, the
 * store does not), so returning to the Device tab does not re-list the card.
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
     * failure, so call sites decide what a failure means. A `busy` from the client marks the
     * store's `busy` flag on the way through.
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
     * Load the three lists, once per `(serial, epoch)`. A remount of the page on the same device
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
            // One listing, filtered here rather than three filtered by the device. §3.3's kind
            // filter exists, but three round trips buy nothing when the whole catalog fits in a
            // couple of pages and the page wants all three kinds every time.
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
            // ends it (§3.5), so it is listed and not offered: the page shows it as recording.
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
     * Every route id that is a stage of some trip — the rows the top-level list leaves out.
     *
     * A `number`, not a `bigint`, because a trip object names its stages as `u16` — see
     * {@link stagesOf}.
     */
    get stagedIds(): Set<number> {
        const staged = new Set<number>();
        for (const trip of this.trips) for (const id of trip.detail?.stages ?? []) staged.add(id);
        return staged;
    }

    /** Routes that are not inside any trip, in list order. */
    get topLevelRoutes(): CatalogEntry[] {
        const staged = this.stagedIds;
        return this.routes.filter((route) => !staged.has(Number(route.objectId)));
    }

    /**
     * A trip's stages resolved against the route list. Null marks a dangling id — a member route
     * deleted on its own, which the device tolerates and serves verbatim.
     *
     * The comparison narrows the catalog's `u64` `ObjectId` to a `number` because the **trip object
     * names its stages in 16 bits** (`objects.ts`). That is a payload-format limit, not a wire one:
     * a card whose id cursor has passed 65,535 can hold routes no trip can name. Nothing here can
     * fix it — the trip object is a device format — and `manage.ts` refuses to build such a trip
     * rather than writing an id that truncates.
     */
    stagesOf(trip: TripView): Array<{ id: number; route: CatalogEntry | null }> {
        const byId = new Map(this.routes.map((route) => [Number(route.objectId), route]));
        return (trip.detail?.stages ?? []).map((id) => ({ id, route: byId.get(id) ?? null }));
    }

    /** True where this entry is a ride the device is still recording (§3.5 refuses to serve one). */
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
