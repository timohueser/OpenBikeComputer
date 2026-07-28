/**
 * Renaming routes and editing trips — the mutations the device page offers beyond delete.
 *
 * Neither is a protocol feature, and that is the point: both ride on the one primitive the wire
 * already has, **upload to an existing id replaces the object atomically** (client.ts). A rename
 * downloads the OBCR, rewrites the 48-byte name field, and puts the same object back under the
 * same id — so its retention clock, its expiry and every reference to it survive. A trip edit
 * downloads a 56-byte-plus-two-per-stage object, mutates the stage list, and does the same.
 *
 * Nothing here talks to the store: callers run these inside `dashboard.enqueue` (each call is one
 * or two transfers) and refresh afterwards.
 */

import type { ProtocolClient } from "../usb/client";
import { decodeTripObject, encodeTripObject, type TripObject } from "../usb/objects";
import { NEW_OBJECT_ID, ObjectType } from "../usb/protocol";
import { decodeRouteHeader, ROUTE_NAME_MAX, truncateUtf8 } from "./route";

/** The trip name field's cap — same 48-byte field as a route's (`objects.ts` §7.7). */
export const TRIP_NAME_MAX = 48;

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

/** Download, rewrite the name, replace at the same id. The object id — and with it the route's
 *  retention and expiry — is exactly what this dance preserves. */
export async function renameRoute(
    client: ProtocolClient,
    objectId: number,
    name: string,
    signal?: AbortSignal,
): Promise<void> {
    const obcr = await client.download(ObjectType.Route, objectId, { signal });
    await client.upload(ObjectType.Route, objectId, renameRouteBytes(obcr, name), { signal });
}

/** Create a trip over existing routes. Returns the id the device assigned. */
export async function createTrip(
    client: ProtocolClient,
    name: string,
    stages: readonly number[],
    signal?: AbortSignal,
): Promise<number> {
    const bytes = encodeTripObject({ name: cleanName(name, TRIP_NAME_MAX, "Trip"), stages: [...stages] });
    const { objectId } = await client.upload(ObjectType.Trip, NEW_OBJECT_ID, bytes, { signal });
    return objectId;
}

/**
 * Edit a trip: download, apply `mutate`, replace at the same id. Returns what was written.
 *
 * The read-modify-write is not raced against the device — it cannot change a trip on its own mid
 * session — but it *is* serialized against this page's other operations by the caller's queue.
 */
export async function updateTrip(
    client: ProtocolClient,
    objectId: number,
    mutate: (trip: TripObject) => TripObject,
    signal?: AbortSignal,
): Promise<TripObject> {
    const current = decodeTripObject(await client.download(ObjectType.Trip, objectId, { signal }));
    const next = mutate(current);
    const written: TripObject = { ...next, name: cleanName(next.name, TRIP_NAME_MAX, "Trip") };
    await client.upload(ObjectType.Trip, objectId, encodeTripObject(written), { signal });
    return written;
}

// --- pure stage-list mutators, for `updateTrip` --------------------------------

export function addStage(trip: TripObject, routeId: number): TripObject {
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
