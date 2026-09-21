/**
 * The managed ride library, and the pull that fills it.
 *
 * The counterpart to `rides.ts`, which is the hosted tier's one-shot GPX export and is deliberately
 * a dead end. This is the real product for Android and no-phone riders: rides land in a folder they
 * can see, back up and drag into anything.
 *
 * Nothing in this file is Tauri-aware. The {@link RideLibrary} it writes through is an interface
 * whose only shipped implementation is `lib/desktop/library.ts`, and whose other implementation is
 * the fake in `library.test.ts`. That split is what lets the two rules below be tested as behaviour
 * rather than as a mocked call sequence.
 *
 * **Always pull the full ride list and dedupe locally.** What decides whether a ride is fetched is
 * whether *this library* already holds it, never anything the device says about it. Pulling twice is
 * then a no-op by construction. `LIST` makes that cheap: a catalog page is metadata, and nothing is
 * downloaded to decide what to download.
 *
 * **The key is `(serial, StoreId, ObjectId)`.** An `ObjectId` is never reused within one card, but a
 * re-initialized card mints a new `StoreId` and starts its ids again, so a bare id names two
 * different rides on either side of one. The iOS companion uses the same key, so the two libraries
 * agree about what "the same ride" is.
 *
 * A possession acknowledgement is not on the USB cable: it changes no object and therefore has no
 * store meaning, so it keeps the BLE control surface it already had. A rider who syncs only over the
 * cable gets their rides copied and the device is not told. The ordering discipline that ack needed
 * — import resolves only after fsync — is kept anyway, because it is what makes
 * {@link PullReport} true.
 */

import { trackToGpx } from "../convert/bridge";
import { Crc32 } from "../usb/crc32";
import { decodeRideObject, encodeRideObject, type RideObject } from "../usb/objects";
import type { CatalogEntry } from "../usb/protocol";
import {
    RideExportError,
    recordedRides,
    rideKey,
    type RideScope,
    type RideSource,
} from "./rides";
import type { JobContext } from "./progress";

export type { CatalogEntry, RideObject, RideScope };

/**
 * One ride in the library, as the index stores it.
 *
 * Mirrors `rides::LibraryRide` in `apps/obc-desktop/src/rides.rs` field for field. `present` and
 * `gpxPresent` are recomputed against the filesystem on every read there, so they describe the disk
 * now rather than when the entry was written. The two paths point at different places: `ridePath` is
 * the archived `.obcride` in **app data**, `gpxPath` the `.gpx` in the **visible** folder, and
 * `present` means the archive file exists.
 */
export interface LibraryRide {
    readonly key: string;
    readonly serial: string;
    /** The complete lowercase 32-digit StoreId hex string. */
    readonly storeId: string;
    readonly objectId: bigint;
    readonly name: string;
    /** Ride start, unix seconds UTC. `0` on a device that never had a trusted clock. */
    readonly startTime: number;
    readonly distanceM: number;
    readonly movingTimeS: number;
    readonly climbM: number;
    readonly points: number;
    readonly bytes: number;
    readonly crc32: number;
    /** When this app first landed the ride. Never re-stamped by a second pull. */
    readonly importedAt: number;
    readonly ridePath: string;
    readonly gpxPath: string;
    /** The downsampled `[lat, lon]` preview, in degrees — drawn from the ride's own points. */
    readonly track: readonly (readonly [number, number])[];
    readonly present: boolean;
    readonly gpxPresent: boolean;
}

/** One ride, on its way into the library. */
export interface RideImport {
    readonly serial: string;
    readonly storeId: string;
    readonly objectId: bigint;
    readonly name: string;
    readonly startTime: number;
    readonly distanceM: number;
    readonly movingTimeS: number;
    readonly climbM: number;
    readonly points: number;
    readonly crc32: number;
    readonly track: readonly (readonly [number, number])[];
    /** The ride object exactly as it came off the wire — the lossless archive. */
    readonly object: Uint8Array;
    readonly gpx: string;
}

/** Where the library is, and what is in it. */
export interface LibraryView {
    readonly folder: string;
    /** False once the rider has relocated it. Only affects what the UI says. */
    readonly isDefault: boolean;
    readonly rides: readonly LibraryRide[];
}

/**
 * The managed folder, as this module needs it.
 *
 * Every method is a promise because the only implementation is a filesystem behind an IPC boundary,
 * and because {@link import} has to be awaited for its durability to mean anything.
 */
export interface RideLibrary {
    view(): Promise<LibraryView>;
    /**
     * Land one ride durably. **Resolves only after fsync** — of the ride object, of the GPX, and of
     * the index that names them. Idempotent on `(serial, storeId, id)`: a second import of a ride
     * already held writes nothing and does not move its `importedAt`.
     */
    import(ride: RideImport): Promise<{ ride: LibraryRide; imported: boolean }>;
    /** The stored ride object of one key — what a GPX re-export decodes. */
    readObject(key: string): Promise<Uint8Array>;
    /** (Re-)write one ride's GPX. Resolves to where it went. */
    writeGpx(key: string, gpx: string): Promise<string>;
    /** Show a file (or the folder) in the OS file manager. */
    reveal(path: string): Promise<void>;
    /** Open the native chooser and move the library. `null` when the rider dismissed it. */
    chooseFolder(): Promise<string | null>;
}

/**
 * - `no-scope` — the device reported no serial, or no `StoreId`. Ids from it cannot be keyed to an
 *   era, so nothing is imported.
 */
export type RideLibraryErrorCode = "no-scope";

export class RideLibraryError extends Error {
    readonly code: RideLibraryErrorCode;

    constructor(code: RideLibraryErrorCode, message: string) {
        super(message);
        this.name = "RideLibraryError";
        this.code = code;
    }
}

/** One ride the pull could not land, and why. The rest of the batch still lands. */
interface RideFailure {
    readonly objectId: bigint;
    readonly name: string;
    readonly message: string;
}

export interface PullReport {
    /** Rides the device listed — the *whole* catalog, always. */
    readonly listed: number;
    /** Rides new to this library. */
    readonly imported: readonly LibraryRide[];
    /**
     * Rides the library had a *record* of but not the file — most often because the rider deleted
     * something in the file manager. Re-downloaded and re-written; their `importedAt` is untouched,
     * so they are not new and are not counted as such.
     */
    readonly repaired: readonly LibraryRide[];
    /** Rides the library already held whole, so nothing was downloaded. */
    readonly alreadyHeld: number;
    readonly failed: readonly RideFailure[];
    /** Rides the device is still recording, which it refuses to serve. Named, never silent. */
    readonly recording: number;
}

/**
 * Pull every ride the device does not already hold a durable copy of here.
 *
 * The order is the contract:
 *
 * 1. list the whole ride catalog, unconditionally, minus what is still recording;
 * 2. dedupe locally by `(serial, StoreId, ObjectId)` against the library's own index;
 * 3. download, decode, convert and import each missing ride one at a time, each import resolving
 *    only after its fsync.
 *
 * A ride that fails at step 3 is reported and skipped; the others still land.
 */
export async function pullRides(
    source: RideSource,
    library: RideLibrary,
    scope: RideScope,
    ctx: JobContext,
): Promise<PullReport> {
    requireScope(scope);

    ctx.phase("reading");
    // Rule 1: the full catalog, every time. There is no "what's new" query and there must not be
    // one — the device does not know what this library holds.
    const listed = await source.listRides(ctx.signal);
    const available = recordedRides(listed);

    // Rule 2: dedupe here, by the composite key. `present` is part of the test on purpose — a record
    // whose ride object the rider deleted is not a durable copy, so it is fetched again.
    //
    // `gpxPresent` deliberately is *not*: a missing GPX is a derived file and the archive it comes
    // from is right there, so the logbook's auto-repair regenerates it locally instead
    // ({@link reexportGpx}).
    const held = new Map((await library.view()).rides.map((ride) => [ride.key, ride]));
    const wanted = [...available]
        // Oldest first, by `ObjectId`. A `LIST` entry carries no start time, and the id is a
        // monotonic allocation cursor — so on one card, id order *is* recording order. It is a proxy,
        // and it is the only one the catalog offers.
        .sort((a, b) => (a.objectId < b.objectId ? -1 : a.objectId > b.objectId ? 1 : 0))
        .filter((entry) => !held.get(rideKey(scope, entry.objectId))?.present);

    const imported: LibraryRide[] = [];
    const repaired: LibraryRide[] = [];
    const failed: RideFailure[] = [];
    for (const entry of wanted) {
        ctx.signal.throwIfAborted();
        try {
            const landed = await importRide(source, library, scope, entry, ctx);
            // Everything in `wanted` was missing something, so a ride the library reports as not
            // new was a record without its file — repaired rather than imported.
            (landed.imported ? imported : repaired).push(landed.ride);
        } catch (cause) {
            if (ctx.signal.aborted) throw cause;
            failed.push({
                objectId: entry.objectId,
                name: entry.displayName || `Ride ${entry.objectId}`,
                message: cause instanceof Error ? cause.message : String(cause),
            });
        }
    }

    ctx.phase("done");
    return {
        listed: available.length,
        imported,
        repaired,
        alreadyHeld: available.length - wanted.length,
        failed,
        recording: listed.length - available.length,
    };
}

/** Both halves of the era, or nothing is copied. */
function requireScope(scope: RideScope): asserts scope is RideScope & { storeId: string } {
    if (!scope.serial || scope.storeId === null) {
        throw new RideLibraryError(
            "no-scope",
            "This device did not report both a serial number and a card identity, so its ride ids " +
                "cannot be told apart from another device's. Nothing was copied.",
        );
    }
}

/** Pull one ride and land it. Split out so a single-ride retry is the same code path. */
async function importRide(
    source: RideSource,
    library: RideLibrary,
    scope: RideScope & { storeId: string },
    entry: CatalogEntry,
    ctx: JobContext,
): Promise<{ ride: LibraryRide; imported: boolean }> {
    ctx.phase("downloading", Number(entry.payloadLength));
    const object = await source.downloadRide(entry, {
        signal: ctx.signal,
        onProgress: (done: number, total: number) => ctx.progress(done, total),
    });

    ctx.phase("converting", object.length);
    // The client verified the whole-payload CRC before returning, so a decode failure here is a
    // *format* disagreement — firmware newer than this build — and needs the other sentence.
    let ride: RideObject;
    try {
        ride = decodeRideObject(object);
    } catch (cause) {
        throw new RideExportError(
            "unreadable-ride",
            "That ride arrived intact but this build cannot read it. That usually means the device " +
                "is running newer firmware than the app.",
            { cause },
        );
    }
    if (ride.points.length === 0) {
        throw new RideExportError(
            "empty-ride",
            "That ride has no recorded points — it was probably stopped before the device had a fix.",
        );
    }

    const gpx = await gpxOf(ride);
    ctx.phase("verifying");
    // Everything below this line is the durable write.
    //
    // Every field but the id and the name comes from the **payload**: a `LIST` entry carries id,
    // revision, length, CRC, kind, flags and a display name, so the distance, the duration and the
    // start time exist only inside the ride object. This path downloads the object anyway; what is
    // gone is the ability to show those figures *before* downloading.
    return library.import({
        serial: scope.serial,
        storeId: scope.storeId,
        objectId: entry.objectId,
        name: ride.name || entry.displayName,
        startTime: ride.startTime,
        distanceM: ride.distanceM,
        movingTimeS: ride.movingTimeS,
        climbM: ride.climbM,
        points: ride.points.length,
        // The device's own CRC-32 over the same bytes, kept in the index so the archive can be
        // re-checked without the device. It is also what a lost create is reconciled against.
        crc32: Crc32.of(object),
        track: previewTrack(ride),
        object,
        gpx,
    });
}

/** Pull one ride: the per-row pull, same code path as the bulk one. */
export async function pullRide(
    source: RideSource,
    library: RideLibrary,
    scope: RideScope,
    entry: CatalogEntry,
    ctx: JobContext,
): Promise<{ ride: LibraryRide; imported: boolean }> {
    requireScope(scope);
    const result = await importRide(source, library, scope, entry, ctx);
    ctx.phase("done");
    return result;
}

/**
 * The GPX, from the same `obc_route::track_to_gpx` the device runs at Finish.
 *
 * Through the wasm bridge over the same finished bytes the device serves, pinned byte-for-byte
 * against `specs/vectors/track-export.gpx`. There is no TypeScript GPX writer in this app and there
 * must never be one — a library whose files disagreed with the device's own export would be a
 * slow-burning support problem.
 */
export async function gpxOf(ride: RideObject): Promise<string> {
    return trackToGpx(encodeRideObject(ride), ride.name);
}

/** Re-export one library ride's GPX from its stored object. The auto-repair is this, in a loop. */
export async function reexportGpx(library: RideLibrary, ride: LibraryRide): Promise<string> {
    const object = await library.readObject(ride.key);
    return library.writeGpx(ride.key, await gpxOf(decodeRideObject(object)));
}

/**
 * How many points a stored preview keeps.
 *
 * The list draws a track a couple of hundred pixels wide, so more than this is index weight nobody
 * can see — and the index is read on every open. The Rust side enforces the same number, because a
 * ceiling only one end believes in is not a ceiling.
 */
export const PREVIEW_POINTS = 256;

/**
 * Downsample any `[lat, lon]` track to at most {@link PREVIEW_POINTS}, rounded to six decimals.
 *
 * Uniform stride rather than Douglas–Peucker: this is a thumbnail, the input is already a recorded
 * track, and a stride cannot introduce a shortcut across a switchback the way a tolerance-based
 * simplifier can. The first and last points are always kept. Shared by the library index and the
 * device page's thumbnail store, so a ride thumbnail is the same points in both places.
 */
export function downsampleTrack(points: readonly (readonly [number, number])[]): Array<[number, number]> {
    if (points.length === 0) return [];
    // At or under the cap nothing is dropped — which also makes a second pass a no-op, so a track
    // that went through the ride library's downsample once is not thinned again by the thumb store.
    const stride = points.length <= PREVIEW_POINTS ? 1 : Math.ceil(points.length / (PREVIEW_POINTS - 1));
    const out: Array<[number, number]> = [];
    for (let i = 0; i < points.length; i += stride) {
        out.push([round6(points[i][0]), round6(points[i][1])]);
    }
    const last = points[points.length - 1];
    const tail: [number, number] = [round6(last[0]), round6(last[1])];
    if (out.length === 0 || out[out.length - 1][0] !== tail[0] || out[out.length - 1][1] !== tail[1]) {
        out.push(tail);
    }
    return out;
}

/** A downsampled `[lat, lon]` track for the list — {@link downsampleTrack} over a ride's points. */
export function previewTrack(ride: RideObject): Array<[number, number]> {
    return downsampleTrack(ride.points.map((p) => [p.latMicrodegrees / 1e6, p.lonMicrodegrees / 1e6]));
}

/** Six decimals is a ~11 cm grid — the device's own GPX precision, and about a third of the JSON. */
export function round6(deg: number): number {
    return Math.round(deg * 1e6) / 1e6;
}

/** One track fitted into a box by {@link fitTracks}: its path, and where it starts and ends. */
export interface FittedTrack {
    /** `M … L …` path in the `width × height` viewBox. */
    readonly d: string;
    /** Projected `[x, y]` of the first point — the start dot. */
    readonly start: readonly [number, number];
    /** Projected `[x, y]` of the last point — the end dot. */
    readonly end: readonly [number, number];
}

/**
 * Fit one or more `[lat, lon]` tracks into a shared `width × height` box.
 *
 * One projection for the lot — a trip's stages are drawn against common bounds, so where stage 2
 * begins is where stage 1 ended. Equirectangular with a `cos(lat)` correction on longitude, which is
 * the projection a few kilometres of track deserves. A track with fewer than two points maps to
 * `null`.
 */
export function fitTracks(
    tracks: ReadonlyArray<readonly (readonly [number, number])[]>,
    width: number,
    height: number,
    pad = 2,
): Array<FittedTrack | null> {
    const all = tracks.flat();
    if (all.length < 2) return tracks.map(() => null);
    const lats = all.map((p) => p[0]);
    const lons = all.map((p) => p[1]);
    const midLat = (Math.min(...lats) + Math.max(...lats)) / 2;
    const kx = Math.cos((midLat * Math.PI) / 180) || 1e-6;

    const xs = lons.map((lon) => lon * kx);
    const minX = Math.min(...xs);
    const maxX = Math.max(...xs);
    const minY = Math.min(...lats);
    const maxY = Math.max(...lats);
    // A ride that never moved is a dot, not a divide-by-zero.
    const spanX = maxX - minX || 1e-9;
    const spanY = maxY - minY || 1e-9;
    const scale = Math.min((width - 2 * pad) / spanX, (height - 2 * pad) / spanY);
    const offsetX = (width - spanX * scale) / 2;
    const offsetY = (height - spanY * scale) / 2;

    const project = (point: readonly [number, number]): [number, number] => [
        offsetX + (point[1] * kx - minX) * scale,
        // SVG y grows downwards; north is up.
        height - offsetY - (point[0] - minY) * scale,
    ];

    return tracks.map((track) => {
        if (track.length < 2) return null;
        const d = track
            .map((point, i) => {
                const [x, y] = project(point);
                return `${i === 0 ? "M" : "L"}${x.toFixed(1)} ${y.toFixed(1)}`;
            })
            .join("");
        return { d, start: project(track[0]), end: project(track[track.length - 1]) };
    });
}

/**
 * An SVG path for one preview track, fitted to a `width × height` box — {@link fitTracks} for the
 * single-track case. Returns `null` when there is nothing to draw.
 */
export function trackPath(
    track: readonly (readonly [number, number])[],
    width: number,
    height: number,
    pad = 2,
): string | null {
    return fitTracks([track], width, height, pad)[0]?.d ?? null;
}
