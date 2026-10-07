/**
 * Pulling a recorded ride off the device and handing it to the browser as a GPX.
 *
 * This is the hosted tier's **only** ride feature, and it is deliberately a dead end: a file lands
 * in a Downloads folder and nothing else happens anywhere. The managed library is the desktop app's.
 *
 * {@link RideSource} holds exactly two reads: list the catalog, download one ride. There is no
 * delete and no arm — not as discipline, but because the object {@link rideAccess} hands over does
 * not have those properties at compile time *or* at runtime. Widening the type is a change somebody
 * reviews.
 *
 * Identity is `(serial, StoreId, ObjectId)`, never a bare id. An `ObjectId` is never reused within
 * one card, but a re-initialized card mints a new `StoreId` and starts its ids again, so anything
 * this page remembers about a ride is keyed by {@link rideKey} and thrown away when the scope
 * changes. Nothing is persisted.
 *
 * A ride the device is still recording carries `RECORDING` in its `LIST` entry and cannot be
 * fetched: its length and CRC are zero until the commit that ends it. {@link recordedRides} is
 * where that filter lives, so no call site has to remember it.
 *
 * This buffers where the map path streams. A ride object is `84 + 20 × points`, so a full day is
 * about 1.7 MB. The whole-object CRC is only known when the last byte has arrived, so a streamed
 * export would have to write an unverified file the rider could open before it was checked, and the
 * wasm bridge takes `&[u8]` and returns a whole `String` anyway.
 */

import { trackToGpx } from "../convert/bridge";
import type { FlatStoreClient, TransferOptions } from "../usb/client";
import { decodeRideObject, type RideObject } from "../usb/objects";
import { EntryFlags, ObjectKind, type CatalogEntry } from "../usb/protocol";
import type { DeviceInfo } from "../usb/records";
import type { StoreIdentity } from "../usb/session";
import type { JobContext } from "./progress";

export type { CatalogEntry, RideObject };

/**
 * Everything the ride export may do to a device: two reads, and nothing else.
 *
 * Code written against this type cannot reach `remove`, `put` or `arm`, because they are not
 * members. A `FlatStoreClient` is deliberately **not** one of these: {@link rideAccess} is the only
 * way to obtain a `RideSource`.
 */
export interface RideSource {
    /** Every ride entry in the catalog, the whole listing, paged by the client. */
    listRides(signal?: AbortSignal): Promise<readonly CatalogEntry[]>;
    downloadRide(entry: CatalogEntry, options?: TransferOptions): Promise<Uint8Array>;
}

/**
 * The read-only view of a client that the export path is handed.
 *
 * The narrowing is real at runtime as well as in the type: the returned object owns two bound
 * functions and nothing else, so a cast back to `FlatStoreClient` throws rather than quietly
 * working. Frozen so it cannot be grown in place either.
 */
export function rideAccess(client: FlatStoreClient): RideSource {
    return Object.freeze({
        listRides: async (signal?: AbortSignal) =>
            (await client.list({ kind: ObjectKind.Ride, signal })).entries,
        // The entry's own `(ObjectId, Revision)` pair rather than the head, so a listing and the
        // download that follows it name the same bytes even if the card moved in between. The
        // entry's length rides along as the progress bar's denominator: the answer only states one
        // at the end, and a ride arriving with no total would report nothing until it was over.
        downloadRide: async (entry: CatalogEntry, options?: TransferOptions) =>
            (
                await client.get(
                    { objectId: entry.objectId, revision: entry.revision },
                    { ...options, expectedLength: Number(entry.payloadLength) },
                )
            ).bytes,
    });
}

/**
 * The rides a client may actually fetch: everything the catalog holds except what is being recorded.
 *
 * A `GET` of an entry carrying `RECORDING` is refused, because the store has not committed its
 * length or CRC yet. Filtering here is not politeness, it is the only listing a caller can act on.
 */
export function recordedRides(entries: readonly CatalogEntry[]): CatalogEntry[] {
    return entries.filter((entry) => (entry.flags & EntryFlags.Recording) === 0);
}

/** The device and full card identity. A missing card has no usable scope. */
export interface RideScope {
    readonly serial: string;
    readonly storeId: string | null;
}

/** Identity from the device information and the first catalog page. */
export function rideScope(info: DeviceInfo | null, store: StoreIdentity | null): RideScope {
    return { serial: info?.serialNumber ?? "", storeId: store?.storeId ?? null };
}

/** Changes when either the device or its card changes. */
export function scopeKey(scope: RideScope): string {
    return `${scope.serial}:${scope.storeId ?? "no-store"}`;
}

/** Shared with the desktop index: full StoreId hex and decimal u64 ObjectId. */
export function rideKey(scope: RideScope, objectId: bigint): string {
    return `${scopeKey(scope)}:${objectId}`;
}

/**
 * Why an export failed on this side of the wire. Transport and conversion failures keep their own
 * codes; this covers what only a ride can be.
 *
 * - `empty-ride` — a recording with no points, usually stopped before the first fix.
 * - `unreadable-ride` — the object arrived intact and this build cannot decode it. In practice that
 *   means firmware newer than the page, which must not be reported as a broken transfer.
 */
export type RideExportErrorCode = "empty-ride" | "unreadable-ride";

export class RideExportError extends Error {
    readonly code: RideExportErrorCode;

    constructor(code: RideExportErrorCode, message: string, options?: { cause?: unknown }) {
        super(message, options);
        this.name = "RideExportError";
        this.code = code;
    }
}

function describe(cause: unknown): string {
    return cause instanceof Error ? cause.message : String(cause);
}

/** A ride, converted and ready to hand to the browser. Nothing here is written anywhere by this
 *  module — the caller saves it, or does not. */
interface ExportedRide {
    /** What the file should be called in a Downloads folder. */
    readonly filename: string;
    /** The GPX 1.1 document, exactly as the native exporter would have written it. */
    readonly gpx: string;
    /** Points in the track — what the rider is actually getting. */
    readonly points: number;
    /** Bytes pulled off the device (the ride object, not the GPX). */
    readonly bytes: number;
}

/**
 * Pull one ride and convert it to GPX. The device is not touched in any other way.
 *
 * The CRC is not this function's job to check and is checked all the same: the client verifies the
 * whole-payload CRC before returning, so there is no path from a corrupt transfer to a file offered
 * to the rider.
 */
export async function exportRide(source: RideSource, entry: CatalogEntry, ctx: JobContext): Promise<ExportedRide> {
    ctx.phase("downloading", Number(entry.payloadLength));
    const bytes = await source.downloadRide(entry, {
        signal: ctx.signal,
        onProgress: (done, total) => ctx.progress(done, total),
    });

    ctx.phase("converting", bytes.length);
    // The bytes are known good — `download` verified the whole-object CRC — so a decode failure here
    // is a *format* disagreement, not a broken transfer, and the rider needs the other sentence:
    // this page is behind the device.
    let ride: RideObject;
    try {
        ride = decodeRideObject(bytes);
    } catch (cause) {
        throw new RideExportError(
            "unreadable-ride",
            `The ride arrived intact but this page cannot read it (${describe(cause)}). That usually ` +
                "means the device is running newer firmware than this page — reload and try again.",
            { cause },
        );
    }
    if (ride.points.length === 0) {
        throw new RideExportError(
            "empty-ride",
            "That ride has no recorded points — there is nothing to put in a GPX file. It was " +
                "probably stopped before the device had a fix.",
        );
    }
    const gpx = await trackToGpx(bytes, ride.name);
    ctx.progress(bytes.length, bytes.length);
    return { filename: rideFilename(entry, ride), gpx, points: ride.points.length, bytes: bytes.length };
}

/**
 * What the saved file is called: the ride's start date, then its name.
 *
 * The date leads because a Downloads folder sorts by name and rides are read in order; the name
 * follows because the rider chose it. The date is formatted in **UTC**, because the ride object's
 * `start_time` is UTC seconds and rendering it locally would put a late evening ride on the wrong
 * day for anyone west of Greenwich. A device that has never had a trusted clock reports `0` and
 * gets no date at all rather than 1970.
 */
export function rideFilename(entry: CatalogEntry, ride?: RideObject): string {
    const name = slug(ride?.name || entry.displayName) || `ride-${entry.objectId}`;
    // The start date comes from the ride **payload**, because a `LIST` entry does not carry one. A
    // caller naming a file before it has downloaded the ride gets the name alone.
    const date = rideDate(ride?.startTime ?? 0);
    return `${date ? `${date}-` : ""}${name}.gpx`;
}

/** The ride's start day as `YYYY-MM-DD` in UTC, or null when the device's clock was never set. */
export function rideDate(startTime: number): string | null {
    if (!startTime) return null;
    return new Date(startTime * 1000).toISOString().slice(0, 10);
}

/** A filename-safe form of a ride name: ASCII-ish, no separators, no runs of punctuation. */
function slug(name: string): string {
    return name
        .normalize("NFKD")
        .replace(/[^A-Za-z0-9]+/g, "-")
        .replace(/^-+|-+$/g, "")
        .slice(0, 48)
        .toLowerCase();
}

/** A ride's moving time as `h:mm` / `m:ss` — the shape a rider reads on a computer, not prose. */
export function rideDuration(seconds: number): string {
    const hours = Math.floor(seconds / 3600);
    const minutes = Math.floor((seconds % 3600) / 60);
    if (hours > 0) return `${hours}:${String(minutes).padStart(2, "0")} h`;
    return `${minutes}:${String(Math.floor(seconds % 60)).padStart(2, "0")} min`;
}

/** A ride's distance in kilometres, at the precision the figure deserves. */
export function rideDistance(metres: number): string {
    return `${(metres / 1000).toFixed(1)} km`;
}
