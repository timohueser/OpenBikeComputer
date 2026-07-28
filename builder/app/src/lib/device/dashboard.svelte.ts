/**
 * The device page's model of what is on the card: routes, trips (with their stage lists), rides.
 *
 * Two rules shape everything here, both inherited from the protocol client:
 *
 *   1. **One transfer at a time, enforced by throwing.** Every `list*` call is itself a transfer
 *      (a download of a singleton list object), so a page that fires three lists and a preview
 *      concurrently would trip the client's own `busy` guard. `enqueue` is the answer: every cable
 *      operation this page makes goes through one promise chain, so the page can never collide
 *      with *itself*. It can still collide with a transfer another surface started — a map send
 *      from the builder tab — and that is not an error to retry into but a state to render:
 *      {@link DeviceDashboard.busy} says so, and the page offers a Retry once the header's
 *      readout shows the slot free.
 *
 *   2. **Ids are only meaningful inside `(serial, epoch)`.** A card swap or reconnect of a
 *      different device makes every cached entry a claim about different objects, so the store
 *      remembers which scope it loaded for and reloads when the scope changes.
 *
 * A module singleton, deliberately: the lists survive a tab switch (the component unmounts, the
 * store does not), so returning to the Device tab does not re-transfer three list objects.
 */

import type { ProtocolClient } from "../usb/client";
import { DeviceError } from "../usb/client";
import { ObjectType } from "../usb/protocol";
import { decodeTripObject, type RideListEntry, type RouteListEntry, type TripListEntry, type TripObject } from "../usb/objects";
import { scopeKey, type RideScope } from "./rides";

/** A trip as the page renders it: the catalog row plus the stage list behind it. `detail` is null
 *  only when the trip object itself could not be read — rendered as a group with no rows. */
export interface TripView extends TripListEntry {
    readonly detail: TripObject | null;
}

export class DeviceDashboard {
    routes = $state<RouteListEntry[]>([]);
    trips = $state<TripView[]>([]);
    rides = $state<RideListEntry[]>([]);
    /** The device listed its newest rides only (spec: the list object truncates). */
    ridesTruncated = $state(false);
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
    async ensureLoaded(client: ProtocolClient, scope: RideScope): Promise<void> {
        const key = scopeKey(scope);
        if (this.loadedFor === key) return;
        this.loadedFor = key;
        await this.refresh(client);
    }

    /** Re-read everything. Called after every mutation — objects are small and the card is the
     *  authority, so re-listing beats mirroring each edit locally and drifting. */
    async refresh(client: ProtocolClient): Promise<void> {
        this.loading = true;
        this.error = null;
        try {
            // Sequential on purpose — each list is a transfer, and the chain is the page's
            // serialization guarantee. Trip details ride along: a trip object is 56 bytes plus
            // two per stage, so "download them all" is smaller than one list header.
            const routes = await this.enqueue(() => client.listRoutes());
            const trips = await this.enqueue(() => client.listTrips());
            const details: TripView[] = [];
            for (const entry of trips.entries) {
                const detail = await this.enqueue(() =>
                    client.download(ObjectType.Trip, entry.objectId),
                ).then(decodeTripObject).catch(() => null);
                details.push({ ...entry, detail });
            }
            const rides = await this.enqueue(() => client.listRides());
            this.routes = routes.entries;
            this.trips = details;
            this.rides = rides.entries;
            this.ridesTruncated = rides.truncated;
        } catch (cause) {
            this.error = cause instanceof Error ? cause.message : String(cause);
            // Nothing loaded is a fact worth retrying, not a scope worth remembering.
            this.loadedFor = null;
        } finally {
            this.loading = false;
        }
    }

    /** Every route id that is a stage of some trip — the rows the top-level list leaves out. */
    get stagedIds(): Set<number> {
        const staged = new Set<number>();
        for (const trip of this.trips) for (const id of trip.detail?.stages ?? []) staged.add(id);
        return staged;
    }

    /** Routes that are not inside any trip, in list order. */
    get topLevelRoutes(): RouteListEntry[] {
        const staged = this.stagedIds;
        return this.routes.filter((r) => !staged.has(r.objectId));
    }

    /** A trip's stages resolved against the route list. Null marks a dangling id — a member route
     *  deleted on its own, which the device tolerates and serves verbatim (objects.ts §7.7). */
    stagesOf(trip: TripView): Array<{ id: number; route: RouteListEntry | null }> {
        const byId = new Map(this.routes.map((r) => [r.objectId, r]));
        return (trip.detail?.stages ?? []).map((id) => ({ id, route: byId.get(id) ?? null }));
    }

    /** Drop everything, for a disconnect: the next connect decides what to load. */
    invalidate(): void {
        this.loadedFor = null;
        this.routes = [];
        this.trips = [];
        this.rides = [];
        this.ridesTruncated = false;
        this.error = null;
        this.busy = false;
    }
}

export const dashboard = new DeviceDashboard();
