/**
 * Renaming routes and editing trips — the mutations the device page offers beyond remove.
 *
 * Neither is a protocol feature, and that is the point: both ride on the one primitive §3.6 already
 * has, **a `PUT` naming an existing object replaces it in one commit**. A rename gets the OBCR,
 * rewrites the 48-byte name field in the payload, and puts the same object back under the same
 * `ObjectId` — so every reference to it survives. A trip edit gets a 56-byte-plus-eight-per-stage
 * object, mutates the stage list, and does the same.
 *
 * **Every replace carries the revision it expects** (§3.6), and that is the substance rather than
 * ceremony: the check runs at admission and again immediately before the commit, so an object
 * something else replaced in between fails the compare-and-swap instead of silently clobbering. The
 * caller supplies the entry it listed; a stale one earns `revisionConflict` and the page re-lists.
 *
 * A rename writes the name in **two** places, because the two are different fields with different
 * readers: the OBCR header's name is what the device shows while navigating, and §3.6's display name
 * is what a catalog listing shows. Writing only one of them would make the device page and the
 * device disagree about what a route is called.
 *
 * Nothing here talks to the store: callers run these inside `dashboard.enqueue` (each call is one or
 * two transfers) and refresh afterwards.
 */

import type { FlatStoreClient } from "../usb/client";
import { truncateUtf8 } from "../format";
import { decodeTripObject, encodeTripObject, type TripObject } from "../usb/objects";
import { ObjectKind, type CatalogEntry, type PutResponse } from "../usb/protocol";
import { decodeRouteHeader, ROUTE_NAME_MAX } from "./route";

/** The trip name field's cap — the same 48-byte field a route's name and §3.6's both use. */
export const TRIP_NAME_MAX = 48;

/**
 * The largest route id a trip can name.
 *
 * Trip v2 stores the flat store's complete nonzero `u64` ObjectId.
 */
export const MAX_TRIP_STAGE_ID = 0xffff_ffff_ffff_ffffn;

/** A route this trip format cannot name. Its own error, because the fix is not "try again". */
export class TripStageError extends Error {
    constructor(objectId: bigint) {
        super(
            `Route ${objectId} cannot be put in a trip: its id is outside the trip format's nonzero ` +
                `u64 range (maximum ${MAX_TRIP_STAGE_ID}).`,
        );
        this.name = "TripStageError";
    }
}

/** Validate an `ObjectId` for the trip object's nonzero `u64` stage field. */
export function stageId(objectId: bigint): bigint {
    if (objectId <= 0n || objectId > MAX_TRIP_STAGE_ID) throw new TripStageError(objectId);
    return objectId;
}

/**
 * The OBCR bytes with a new name in the header: length byte at offset 6, the null-padded 48-byte
 * field at 64 (`OBCR_Spec.md` §1 — the offsets `decodeRouteHeader` reads). The old name is zeroed
 * before the new one lands, because the field is *null-padded*, and a shorter name over a longer
 * one would otherwise leave the tail of the old name in the file.
 */
export function renameRouteBytes(obcr: Uint8Array, name: string): Uint8Array {
    decodeRouteHeader(obcr); // magic, version, length — refuse to "rename" something else
    const bytes = new TextEncoder().encode(cleanName(name, ROUTE_NAME_MAX, "Route"));
    const out = obcr.slice();
    out[6] = bytes.length;
    out.fill(0, 64, 64 + ROUTE_NAME_MAX);
    out.set(bytes, 64);
    return out;
}

/** Get, rewrite the name, replace at the same id. The `ObjectId` — and with it every reference to
 *  the route — is exactly what this dance preserves. */
export async function renameRoute(
    client: FlatStoreClient,
    route: Pick<CatalogEntry, "objectId" | "revision">,
    name: string,
    signal?: AbortSignal,
): Promise<PutResponse> {
    const obcr = await client.get({ objectId: route.objectId, revision: route.revision }, { signal });
    const clean = cleanName(name, ROUTE_NAME_MAX, "Route");
    return client.put(
        {
            objectId: route.objectId,
            expectedRevision: route.revision,
            kind: ObjectKind.Route,
            displayName: clean,
        },
        renameRouteBytes(obcr.bytes, clean),
        { signal },
    );
}

/** Create a trip over existing routes. Returns what the commit published, id included. */
export async function createTrip(
    client: FlatStoreClient,
    name: string,
    stages: readonly bigint[],
    signal?: AbortSignal,
): Promise<PutResponse> {
    const clean = cleanName(name, TRIP_NAME_MAX, "Trip");
    const bytes = encodeTripObject({ name: clean, stages: stages.map(stageId) });
    return client.put({ kind: ObjectKind.Trip, displayName: clean }, bytes, { signal });
}

/**
 * Edit a trip: get, apply `mutate`, replace at the same id. Returns what was written.
 *
 * The read-modify-write is serialized against this page's other operations by the caller's queue,
 * and against everything else by §3.6's compare-and-swap: the expected revision is the one the
 * caller listed, so a trip that moved underneath this returns `revisionConflict` rather than
 * overwriting the change.
 */
export async function updateTrip(
    client: FlatStoreClient,
    trip: Pick<CatalogEntry, "objectId" | "revision">,
    mutate: (trip: TripObject) => TripObject,
    signal?: AbortSignal,
): Promise<TripObject> {
    const current = decodeTripObject(
        (await client.get({ objectId: trip.objectId, revision: trip.revision }, { signal })).bytes,
    );
    const next = mutate(current);
    const written: TripObject = { ...next, name: cleanName(next.name, TRIP_NAME_MAX, "Trip") };
    await client.put(
        {
            objectId: trip.objectId,
            expectedRevision: trip.revision,
            kind: ObjectKind.Trip,
            displayName: written.name,
        },
        encodeTripObject(written),
        { signal },
    );
    return written;
}

// --- pure stage-list mutators, for `updateTrip` --------------------------------

export function addStage(trip: TripObject, routeId: bigint): TripObject {
    // Adding a stage that is already in the trip is a no-op, not a duplicate: the
    // menu offering the add has no way to know the trip's current stages are stale.
    if (trip.stages.includes(routeId)) return trip;
    return { ...trip, stages: [...trip.stages, routeId] };
}

export function removeStage(trip: TripObject, index: number): TripObject {
    return { ...trip, stages: trip.stages.filter((_, i) => i !== index) };
}

/** Move the stage at `index` by `delta` places, clamped to the list. */
export function moveStage(trip: TripObject, index: number, delta: number): TripObject {
    const to = Math.max(0, Math.min(trip.stages.length - 1, index + delta));
    if (to === index || index < 0 || index >= trip.stages.length) return trip;
    const stages = [...trip.stages];
    const [moved] = stages.splice(index, 1);
    stages.splice(to, 0, moved);
    return { ...trip, stages };
}

/** Trimmed, non-empty, inside the field's byte cap. */
function cleanName(name: string, maxBytes: number, fallback: string): string {
    return truncateUtf8(name.trim() || fallback, maxBytes);
}
