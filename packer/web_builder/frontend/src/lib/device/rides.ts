/**
 * Pulling a recorded ride off the device and handing it to the browser as a GPX (C5, #904).
 *
 * This is the hosted tier's **only** ride feature, and it is deliberately a dead end: a file lands
 * in a Downloads folder and nothing else happens anywhere. The managed library — a folder, a list,
 * a record of what has been kept — is the desktop app's (E2 #912), and the difference is not a
 * feature gap. It is what decides whether this path is allowed to ack.
 *
 * ## The one rule: the browser never acks
 *
 * `synced` on the device does not mean "the phone has it". Read where it is used — the delete
 * guard, the warning cue on the Rides list, and the auto-expiry anchor (#638) — and it means
 * **"a durable copy of this ride exists off the device"**. It is a durability predicate. Setting it
 * writes a `synced_at` stamp into `/tracks/SYNCED.SET`, and that stamp is the moment a countdown
 * starts against the only copy of a ride.
 *
 * A browser download is not durable. The rider can cancel at the save dialog, the disk can be full,
 * the tab can close between the transfer and the write. #894 therefore locks three sinks at three
 * levels of trust — the phone acks and heals from its own library, the desktop app acks *after
 * fsync*, and **the browser never acks, on any path, under any circumstance**.
 *
 * So this file does not merely omit the call. {@link RideSource} is the entire surface the export
 * path is given, and it holds exactly two reads: list the catalog, download one ride. There is no
 * `ackRides`, no `deleteObject`, no generic `command` — not as discipline, but because the object
 * handed in by {@link rideAccess} does not have those properties at compile time *or* at runtime.
 * A future edit that wants to write to the device has to widen a type first, which is a change
 * somebody reviews. `rides.test.ts` pins the behaviour from the other side: a full list-and-export
 * session leaves the device's command log empty and its synced sidecar byte-identical.
 *
 * ## Two more rules inherited from the epic
 *
 * - **Never consult the device's `synced` flag when deciding what to fetch.** It cannot be consulted
 *   even by accident here: the `rideList` entry (spec §7.4, 72 bytes) carries no synced field at
 *   all. Reconciliation only ever travels host → device. The full catalog is listed, always.
 * - **Identity is `(serial, epoch, id)`, never a bare id.** Object ids are recycled after a store
 *   epoch bump — a reformatted card, a factory reset — so anything this page remembers about a ride
 *   is keyed by {@link rideKey} and thrown away when the scope changes. Nothing is persisted; the
 *   scope exists so that what the page holds *within* one visit cannot survive a card swap.
 *
 * ## Why this buffers where the map path streams
 *
 * C4 streams a map through a scratch file because a regional `.obcm` is hundreds of megabytes. A
 * ride is not that: the object is `31 + name + 18 × points` (§7.2), so a 12-hour ride logged at
 * 1 Hz is about 780 KB and a full day is 1.5 MB — two orders of magnitude below the case that
 * forced staging. Two things then argue against streaming rather than merely permitting the buffer:
 * the whole-object CRC-32 is only known when the last byte has arrived, so a streamed export would
 * have to write an unverified file the rider could open before it was checked; and A2's wasm bridge
 * takes `&[u8]` and returns a whole `String`, so the conversion has no streaming form to feed. The
 * peak is the wire object, the transcoded log, and the GPX text — tens of megabytes at the extreme,
 * which is a tab, not a problem.
 */

import { trackToGpx } from "../convert/bridge";
import type { ProtocolClient, TransferOptions } from "../usb/client";
import { decodeRideObject, type RideListEntry, type RideObject } from "../usb/objects";
import { ObjectType } from "../usb/protocol";
import type { VersionRead } from "../usb/protocol";
import type { DeviceInfo } from "../usb/transport";
import type { JobContext } from "./progress";

export type { RideListEntry, RideObject };

// --- the narrowed device surface ----------------------------------------------

/** A ride catalog as the device reports it, with the truncation flag intact (§7.4). */
export interface RideCatalog {
    readonly entries: readonly RideListEntry[];
    /** The device dropped `total - count` older entries at its cap. Surfaced, never hidden. */
    readonly truncated: boolean;
}

/**
 * Everything the browser's ride export may do to a device: two reads, and nothing else.
 *
 * This is the structural half of "never acks" (see the file header). Code written against this type
 * cannot reach `ackRides`, `deleteObject`, `setClock` or the generic `command`, because they are
 * not members — widening it is the only way to write to the device from here, and that is a change
 * somebody reviews rather than an autocomplete accident.
 *
 * A `ProtocolClient` is deliberately **not** one of these: `downloadRide` narrows `download` to the
 * ride namespace and exists nowhere else, so {@link rideAccess} is the only way to obtain a
 * `RideSource`. There is no "just pass the client in" shortcut for a call site to reach for.
 */
export interface RideSource {
    listRides(options?: TransferOptions): Promise<RideCatalog>;
    downloadRide(objectId: number, options?: TransferOptions): Promise<Uint8Array>;
}

/**
 * The read-only view of a client that the export path is handed.
 *
 * The narrowing is real at runtime as well as in the type: the returned object owns two bound
 * functions and nothing else, so `(source as ProtocolClient).ackRides(...)` throws rather than
 * quietly working the day someone reaches for a cast. Frozen so it cannot be grown in place either.
 */
export function rideAccess(client: ProtocolClient): RideSource {
    return Object.freeze({
        listRides: (options?: TransferOptions) => client.listRides(options),
        downloadRide: (objectId: number, options?: TransferOptions) =>
            client.download(ObjectType.Ride, objectId, options),
    });
}

// --- ride identity -------------------------------------------------------------

/**
 * The id era a ride id is meaningful in: the device's serial and its store epoch.
 *
 * `epoch` is `null` on a device with no mounted card, which serves the short identity read — that
 * is "no epoch", never epoch `0`, because `0` is a legal era. A null epoch means ids cannot be
 * trusted to mean anything across a replug, so it gets its own key rather than collapsing to `0`.
 */
export interface RideScope {
    readonly serial: string;
    readonly epoch: number | null;
}

/** The scope of the currently connected device, from the two reads every connection already does. */
export function rideScope(info: DeviceInfo | null, identity: VersionRead | null): RideScope {
    return { serial: info?.serialNumber ?? "", epoch: identity?.storeEpoch ?? null };
}

/** A scope's own key — compare two of these to know whether every remembered id just became
 *  meaningless (a card swap, a different device on the same page). */
export function scopeKey(scope: RideScope): string {
    return `${scope.serial}:${scope.epoch ?? "no-store"}`;
}

/** `(serial, epoch, id)` — the epic's ride identity. A bare id is wrong: ids are recycled after a
 *  store-epoch bump, so two different rides can share one. */
export function rideKey(scope: RideScope, objectId: number): string {
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
export interface ExportedRide {
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
 * The CRC is not this function's job to check and is checked all the same: `download` folds the
 * whole-object CRC-32 as slices arrive and rejects the object before returning, so there is no path
 * from a corrupt transfer to a file offered to the rider. "Exported" means the bytes were intact.
 */
export async function exportRide(source: RideSource, entry: RideListEntry, ctx: JobContext): Promise<ExportedRide> {
    ctx.phase("downloading", entry.byteLen);
    const bytes = await source.downloadRide(entry.objectId, {
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
    const gpx = await trackToGpx(rideToTrackLog(ride), ride.name);
    ctx.progress(bytes.length, bytes.length);
    return { filename: rideFilename(entry, ride), gpx, points: ride.points.length, bytes: bytes.length };
}

// --- ride object -> the log the GPX exporter reads --------------------------------

/*
 * The exporter is `obc_route::track_to_gpx`, reached through A2's wasm bridge — the same code the
 * device and the CLI run, so the file a visitor saves is the file the device would have written.
 * There is no TypeScript GPX writer here and there must never be one.
 *
 * That exporter reads the device's **recorded track log**: a headerless array of fixed 20-byte
 * records (`obc-formats/src/track.rs`). What crosses the wire is the **ride object** (§7.2), which
 * is what the device keeps — the log is a temp file, converted at Finish by `track_to_ride` and
 * deleted. So the pull side has to undo that conversion, and the interesting question is what it
 * costs.
 *
 * `track_to_ride` does exactly four things to each point, and three of them invert exactly:
 *
 * | Field | Device (log -> ride object) | Here (ride object -> log) |
 * | :-- | :-- | :-- |
 * | `lat` / `lon` | µdeg × 10 -> 1e-7 ° | ÷ 10 — **lossless**, every value it wrote is a multiple of 10 |
 * | `ele` | carried verbatim | carried verbatim |
 * | `hr` / `cad` / `pwr` | 1:1, sentinel for absent | 1:1, same sentinels |
 * | `t_ms` | `(t_ms - t0) / 1000` | × 1000 — sub-second resolution is gone, and unused: the exporter writes no `<time>` |
 *
 * And one thing that does **not** come back: `segment_start`. The ride object has no segment flag,
 * so a ride recorded with a pause exports as one `<trkseg>` where the device's own Finish-time GPX
 * would have had two. That loss happens on the device, at Finish, to every peer — the phone's
 * exporter (`GPXRideEncoder.swift`) says the same thing in the same words. It is a property of the
 * wire format, not of this code, and `rides.test.ts` pins it as the *only* difference from the
 * checked-in `track-export.gpx`.
 */

const TRACK_RECORD_LEN = 20;
const TRACK_HR_NONE = 0xff;
const TRACK_CAD_NONE = 0xff;
const TRACK_PWR_NONE = 0xffff;

/** The largest whole-second offset that still fits the log's `t_ms u32` (~49.7 days). */
const MAX_OFFSET_S = Math.floor(0xffffffff / 1000);

/**
 * Re-cast a decoded ride object as the fixed-record log the GPX exporter reads.
 *
 * `eleM` is `null` only for an encoder that is not the firmware — the device stamps every point,
 * writing `0` before its first barometer sample and never the `INT16_MIN` sentinel. The log has no
 * absent representation and `track_to_gpx` always writes an `<ele>`, so a null takes the device's
 * own "no barometer yet" value rather than a fabricated altitude or a sentinel rendered as `-32768`.
 */
export function rideToTrackLog(ride: RideObject): Uint8Array {
    const out = new Uint8Array(ride.points.length * TRACK_RECORD_LEN);
    const view = new DataView(out.buffer);
    ride.points.forEach((point, i) => {
        const at = i * TRACK_RECORD_LEN;
        view.setInt32(at, Math.round(point.lon1e7 / 10), true);
        view.setInt32(at + 4, Math.round(point.lat1e7 / 10), true);
        view.setInt16(at + 8, point.eleM ?? 0, true);
        // flags stay 0: the wire carries no segment breaks, and `track_to_gpx` opens a `<trkseg>`
        // on the first point regardless.
        view.setUint16(at + 10, 0, true);
        view.setUint32(at + 12, Math.min(point.tOffsetS, MAX_OFFSET_S) * 1000, true);
        out[at + 16] = point.hrBpm ?? TRACK_HR_NONE;
        out[at + 17] = point.cadenceRpm ?? TRACK_CAD_NONE;
        view.setUint16(at + 18, point.powerW ?? TRACK_PWR_NONE, true);
    });
    return out;
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
export function rideFilename(entry: RideListEntry, ride?: RideObject): string {
    const name = slug(ride?.name || entry.name) || `ride-${entry.objectId}`;
    const date = rideDate(entry.startTime);
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
