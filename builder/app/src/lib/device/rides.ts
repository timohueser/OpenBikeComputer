/**
 * Pulling a recorded ride off the device and handing it to the browser as a GPX (C5, #904).
 *
 * This is the hosted tier's **only** ride feature, and it is deliberately a dead end: a file lands
 * in a Downloads folder and nothing else happens anywhere. The managed library — a folder, a list,
 * a record of what has been kept — is the desktop app's (E2 #912).
 *
 * ## The surface this path is given
 *
 * {@link RideSource} is the whole of it, and it holds exactly two reads: list the catalog, download
 * one ride. There is no delete and no arm — not as discipline, but because the object
 * {@link rideAccess} hands over does not have those properties at compile time *or* at runtime. A
 * future edit that wants to write to the device has to widen a type first, which is a change
 * somebody reviews.
 *
 * The ride acknowledgement this file used to be organised around is **gone from the cable**.
 * `FLAT_Store_Protocol.md` §5.2.2 retires the v1 `command` selector: a possession ack has no store
 * meaning — it changes no object — so it keeps the BLE control surface it already had and USB does
 * not carry it. What that changes here is one thing and no more: nothing on this path tells the
 * device anything, which was already true of the browser tier and is now true of every USB peer.
 *
 * ## The rule that survives
 *
 * **Identity is `(serial, era, ObjectId)`, never a bare id.** An `ObjectId` is store-global and
 * never reused *within* one card (`FLAT_Store_Format.md` §3), but a re-initialized card mints a new
 * `StoreId` and starts its ids again — so anything this page remembers about a ride is keyed by
 * {@link rideKey} and thrown away when the scope changes. Nothing is persisted; the scope exists so
 * that what the page holds *within* one visit cannot survive a card swap.
 *
 * A ride the device is still recording carries `RECORDING` in its `LIST` entry, and §3.5 refuses a
 * `GET` of one — its length and CRC are zero until the commit that ends it. {@link recordedRides}
 * is where that filter lives, so no call site has to remember it.
 *
 * ## Why this buffers where the map path streams
 *
 * C4 streams a map through a scratch file because a regional `.obcm` is hundreds of megabytes. A
 * ride is not that: the object is `84 + 20 × points` (§7.2), so a 12-hour ride logged at
 * 1 Hz is about 865 KB and a full day is 1.7 MB — two orders of magnitude below the case that
 * forced staging. Two things then argue against streaming rather than merely permitting the buffer:
 * the whole-object CRC-32 is only known when the last byte has arrived, so a streamed export would
 * have to write an unverified file the rider could open before it was checked; and A2's wasm bridge
 * takes `&[u8]` and returns a whole `String`, so the conversion has no streaming form to feed. The
 * peak is the wire object, the transcoded log, and the GPX text — tens of megabytes at the extreme,
 * which is a tab, not a problem.
 */

import { trackToGpx } from "../convert/bridge";
import type { FlatStoreClient, TransferOptions } from "../usb/client";
import { decodeRideObject, type RideObject } from "../usb/objects";
import { EntryFlags, ObjectKind, type CatalogEntry } from "../usb/protocol";
import type { DeviceInfo } from "../usb/records";
import type { StoreIdentity } from "../usb/session";
import type { JobContext } from "./progress";

export type { CatalogEntry, RideObject };

// --- the narrowed device surface ----------------------------------------------

/**
 * Everything the ride export may do to a device: two reads, and nothing else.
 *
 * Code written against this type cannot reach `remove`, `put` or `arm`, because they are not
 * members — widening it is the only way to write to the device from here, and that is a change
 * somebody reviews rather than an autocomplete accident.
 *
 * A `FlatStoreClient` is deliberately **not** one of these: `downloadRide` narrows `get` to the ride
 * kind and exists nowhere else, so {@link rideAccess} is the only way to obtain a `RideSource`.
 */
export interface RideSource {
    /** Every ride entry in the catalog, the whole listing, paged by the client (§3.3). */
    listRides(signal?: AbortSignal): Promise<readonly CatalogEntry[]>;
    downloadRide(entry: CatalogEntry, options?: TransferOptions): Promise<Uint8Array>;
}

/**
 * The read-only view of a client that the export path is handed.
 *
 * The narrowing is real at runtime as well as in the type: the returned object owns two bound
 * functions and nothing else, so `(source as FlatStoreClient).remove(...)` throws rather than
 * quietly working the day someone reaches for a cast. Frozen so it cannot be grown in place either.
 */
export function rideAccess(client: FlatStoreClient): RideSource {
    return Object.freeze({
        listRides: async (signal?: AbortSignal) =>
            (await client.list({ kind: ObjectKind.Ride, signal })).entries,
        // The entry's own `(ObjectId, Revision)` pair rather than the head, so a listing and the
        // download that follows it name the same bytes even if the card moved in between — a
        // revision the device no longer holds is `notFound`, which is the honest answer.
        //
        // The entry's length rides along as the progress bar's denominator: §3.5 only states one in
        // the answer, and a ride arriving with no total would report nothing until it was over.
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
 * §3.5 refuses a `GET` of an entry carrying `RECORDING`, because the store has not committed its
 * length or CRC yet — serving one would report success over an empty payload. A client syncs a ride
 * once that flag has cleared from its `LIST` entry, so filtering here is not politeness, it is the
 * only listing a caller can act on.
 */
export function recordedRides(entries: readonly CatalogEntry[]): CatalogEntry[] {
    return entries.filter((entry) => (entry.flags & EntryFlags.Recording) === 0);
}

// --- ride identity -------------------------------------------------------------

/**
 * The id era a ride id is meaningful in: the device's serial and a fingerprint of the card.
 *
 * `epoch` is `null` where the host could not read a `StoreId` — no card, or a listing that failed.
 * That is "no era", never `0`, because `0` is a legal fingerprint: a client that cannot name the era
 * must fail closed rather than share one bucket with every other cardless device.
 */
export interface RideScope {
    readonly serial: string;
    /** {@link storeEra} of the card's `StoreId`, or `null`. */
    readonly epoch: number | null;
}

/**
 * The card's 128-bit `StoreId` narrowed to the 32 bits the ride index has a column for.
 *
 * The era on the wire is the whole `StoreId` (§3.3), and this throws 96 bits of it away. That is a
 * **cache key and nothing else**: it decides whether a ride the library already holds is the same
 * ride, it authorises nothing, and it is never sent anywhere. Two different cards colliding costs
 * one confused dedupe and has a probability of 2^-32 per pair, against a fleet of one device per
 * rider.
 *
 * The narrowing exists because `apps/obc-desktop/src/rides.rs` stores the era as a `u32` and
 * widening that column is a Rust change this slice does not make. The alternative — keeping the full
 * hex here and letting the desktop index key on something else — would give the two libraries two
 * different answers to "is this the same ride", which is the exact failure the 2026-07-12 incident
 * was.
 */
export function storeEra(storeId: string): number {
    return Number.parseInt(storeId.slice(0, 8), 16) >>> 0;
}

/** The scope of the connected device, from the two reads every connection already does: §5.2.1's
 *  strings and the first `LIST` page's identity prefix. */
export function rideScope(info: DeviceInfo | null, store: StoreIdentity | null): RideScope {
    return { serial: info?.serialNumber ?? "", epoch: store ? storeEra(store.storeId) : null };
}

/** A scope's own key — compare two of these to know whether every remembered id just became
 *  meaningless (a card swap, a different device on the same page). */
export function scopeKey(scope: RideScope): string {
    return `${scope.serial}:${scope.epoch ?? "no-store"}`;
}

/**
 * `(serial, era, ObjectId)` — the ride identity. A bare id is wrong: a re-initialized card
 * starts its ids again, so two different rides can share one across that boundary.
 *
 * `bigint | number` because the two sides that key against each other hold the id differently: a
 * `LIST` entry carries the wire's `u64` and the ride library's index carries a JSON number. They
 * stringify identically, which is the whole reason one key function can serve both — stated here so
 * nobody "fixes" the union by narrowing it and silently splitting the keyspace in two.
 */
export function rideKey(scope: RideScope, objectId: bigint | number): string {
    return `${scopeKey(scope)}:${objectId}`;
}

// --- the export ----------------------------------------------------------------

/**
 * Why an export failed on this side of the wire. Transport and conversion failures keep their own
 * codes (`DeviceError.code`, `ConvertError.code`); this covers what only a ride can be.
 *
 * - `empty-ride` — a recording with no points, usually stopped before the first fix.
 * - `unreadable-ride` — the object arrived intact (its CRC matched) and this build cannot decode
 *   it. In practice that means firmware newer than the page, which is a different sentence from
 *   "the transfer broke" and must not be reported as one.
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
 * The CRC is not this function's job to check and is checked all the same: §3.5 has the device
 * declare the whole-payload CRC in its answer and the client verify it before returning, so there is
 * no path from a corrupt transfer to a file offered to the rider. "Exported" means the bytes were
 * intact.
 */
export async function exportRide(source: RideSource, entry: CatalogEntry, ctx: JobContext): Promise<ExportedRide> {
    ctx.phase("downloading", Number(entry.payloadLength));
    const bytes = await source.downloadRide(entry, {
        signal: ctx.signal,
        onProgress: (done, total) => ctx.progress(done, total),
    });

    ctx.phase("converting", bytes.length);
    // The bytes are known good — `download` verified the whole-object CRC before returning — so a
    // decode failure here is a *format* disagreement, not a broken transfer, and the rider needs
    // the other sentence: this page is behind the device.
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

// --- naming and formatting -------------------------------------------------------

/**
 * What the saved file is called: the ride's start date, then its name.
 *
 * The date leads because a Downloads folder sorts by name and rides are read in order; the name
 * follows because the rider chose it and it is what they will look for. The date is formatted in
 * **UTC**, not the visitor's zone: the ride object's `start_time` is UTC seconds, and rendering it
 * locally would put a late evening ride on the wrong day for anyone west of Greenwich. A device
 * that has never had a trusted clock reports `0` and gets no date at all rather than 1970.
 */
export function rideFilename(entry: CatalogEntry, ride?: RideObject): string {
    const name = slug(ride?.name || entry.displayName) || `ride-${entry.objectId}`;
    // The start date comes from the ride **payload**, because a `LIST` entry does not carry one:
    // §3.3's 88 bytes are id, revision, length, CRC, kind, flags and display name. A caller naming a
    // file before it has downloaded the ride gets the name alone, which is the honest half.
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
