/**
 * An in-memory device: two byte pipes wired to a simulated OBC, speaking the real protocol.
 *
 * **This is a deliverable, not a test helper.** The LM20's USB peripheral does not exist yet
 * (#889), and the epic deliberately does not wait for it: C4 (#903), C5 (#904) and the desktop
 * path (D4 #909) are all built and tested against this. It therefore has to behave like a device
 * rather than like an echo — id assignment, the catalog caps, fresh-upload dedup, the monotonic
 * synced flag, `storeChanged` edges, `busy` on a second transfer, and the abort handshake are all
 * here, because those are the paths a UI gets wrong and only discovers on hardware.
 *
 * Two things it deliberately does **not** do:
 *
 * - **Parse OBCR.** A route crosses the wire as opaque bytes the device writes verbatim (§7.1), so
 *   an uploaded route lists with zeroed metrics and a generated name. A test that cares about
 *   catalog fields {@link MockDevice.seedRoute | seeds} them instead — which is also how the
 *   checked-in `route-list.bin` is served back byte-for-byte.
 * - **Model USB itself.** Timing, stalls and enumeration belong to the WebUSB pipe. What this
 *   models is the *protocol*, plus the one transport property a mock usually fakes away and
 *   shouldn't: the bulk channel hands bytes over in packet-sized slices, so a client that assumes a
 *   read returns a whole logical unit fails here rather than on a rider's desk.
 */

import { ProtocolClient } from "./client";
import { Crc32 } from "./crc32";
import {
    LIST_HEADER_LEN,
    RIDE_ENTRY_LEN,
    ROUTE_ENTRY_LEN,
    TRIP_ENTRY_LEN,
    encodeListHeader,
    encodeRideListEntry,
    encodeRouteListEntry,
    encodeTripListEntry,
    type RideListEntry,
    type RouteListEntry,
    type TripListEntry,
} from "./objects";
import { PipeError, throwIfAborted, type BytePipe, type DeviceLink } from "./pipe";
import {
    Command,
    CommandStatus,
    MAX_RETENTION,
    NEW_OBJECT_ID,
    OBCS_HEADER_LEN,
    OBCS_RECORD_LEN,
    OBCS_VERSION,
    OBCT_HEADER_LEN,
    OBCT_MAGIC,
    OBCT_VERSION,
    ObjectType,
    Op,
    PROTOCOL_VERSION,
    SET_CLOCK_MAX_OFFSET_MIN,
    SET_CLOCK_MIN_UTC,
    SINGLETON_OBJECT_ID,
    TransferStatus,
    decodeConfig,
    decodeTransferControl,
    encodeConfig,
    encodeStatusMessage,
    encodeVersionRead,
    manifestLen,
    viewOf,
    type DeviceConfig,
    type StatusMessage,
    type TransferControl,
} from "./protocol";
import {
    DeviceFrame,
    HostFrame,
    decodeFrame,
    encodeCardFree,
    encodeDeviceInfo,
    encodeFrame,
    type DeviceInfo,
} from "./transport";

// --- the pipe ----------------------------------------------------------------

/** How a channel hands bytes to its reader. */
type ChannelMode = "message" | "stream";

/**
 * One direction of the loopback, with the two transport properties that matter.
 *
 * **Backpressure** is a byte high-water mark: a writer that has filled the channel waits for the
 * reader to drain it. Real backpressure comes from the device NAKing an endpoint it hasn't drained,
 * and a client that queues writes without ever retiring them would outrun any real device. Faking it
 * here is what makes that bug fail in CI. (A *bounded* window of outstanding writes is fine and is
 * what the upload loop does — the high-water mark is what keeps it bounded.)
 *
 * **Segmentation**: a `stream` channel re-slices to `packetSize`, so the reader sees the arbitrary
 * boundaries a bulk endpoint produces. A `message` channel keeps writes whole, which is the control
 * endpoint's contract — one transfer, one frame.
 */
class Channel {
    private readonly chunks: Uint8Array[] = [];
    private queued = 0;
    private readers: Array<{ resolve: (v: Uint8Array) => void; reject: (e: unknown) => void }> = [];
    private writers: Array<() => void> = [];
    private closed = false;

    constructor(
        private readonly mode: ChannelMode,
        private readonly packetSize: number,
        private readonly highWaterMark: number,
    ) {}

    /** Bytes waiting to be read — the backpressure gauge tests assert on. */
    get depth(): number {
        return this.queued;
    }

    async push(bytes: Uint8Array, signal?: AbortSignal): Promise<void> {
        if (this.closed) throw new PipeError("closed", "The loopback pipe is closed.");
        throwIfAborted(signal, "the write");
        // Copy: the caller may reuse its buffer the moment this resolves, and a queued view of a
        // recycled buffer is the classic way a transfer arrives corrupted.
        const owned = bytes.slice();
        if (this.mode === "message") this.enqueue(owned);
        else {
            for (let at = 0; at < owned.length; at += this.packetSize) {
                this.enqueue(owned.subarray(at, Math.min(at + this.packetSize, owned.length)));
            }
        }
        // A writer parked on the high-water mark has to stay cancellable: a cancelled upload whose
        // last `write` is waiting for room would otherwise never observe the abort, and the caller
        // would hang exactly where the UI promised a Cancel button.
        while (this.queued > this.highWaterMark && !this.closed) {
            throwIfAborted(signal, "the write");
            await new Promise<void>((resolve) => {
                this.writers.push(resolve);
                signal?.addEventListener("abort", () => resolve(), { once: true });
            });
        }
        throwIfAborted(signal, "the write");
        if (this.closed) throw new PipeError("closed", "The loopback pipe closed while writing.");
    }

    private enqueue(slice: Uint8Array): void {
        const reader = this.readers.shift();
        if (reader) {
            reader.resolve(slice);
            return;
        }
        this.chunks.push(slice);
        this.queued += slice.length;
    }

    pull(signal?: AbortSignal): Promise<Uint8Array> {
        throwIfAborted(signal, "the read");
        const next = this.chunks.shift();
        if (next) {
            this.queued -= next.length;
            this.wake();
            return Promise.resolve(next);
        }
        if (this.closed) return Promise.reject(new PipeError("closed", "The loopback pipe is closed."));
        return new Promise<Uint8Array>((resolve, reject) => {
            const entry = {
                resolve: (v: Uint8Array) => {
                    signal?.removeEventListener("abort", onAbort);
                    resolve(v);
                },
                reject: (e: unknown) => {
                    signal?.removeEventListener("abort", onAbort);
                    reject(e);
                },
            };
            const onAbort = () => {
                this.readers = this.readers.filter((r) => r !== entry);
                reject(new PipeError("aborted", "The read was cancelled.", { cause: signal?.reason }));
            };
            signal?.addEventListener("abort", onAbort, { once: true });
            this.readers.push(entry);
        });
    }

    /** Drop everything queued and release blocked writers — the pipe-reset primitive. */
    clear(): void {
        this.chunks.length = 0;
        this.queued = 0;
        this.wake();
    }

    close(): void {
        if (this.closed) return;
        this.closed = true;
        const error = new PipeError("closed", "The loopback pipe is closed.");
        while (this.readers.length) this.readers.shift()?.reject(error);
        this.wake();
    }

    private wake(): void {
        const waiting = this.writers;
        this.writers = [];
        for (const resume of waiting) resume();
    }
}

/** One end of a loopback: reads from `inbound`, writes to `outbound`. */
class LoopbackPipe implements BytePipe {
    readonly transport = "loopback";

    constructor(
        private readonly inbound: Channel,
        private readonly outbound: Channel,
    ) {}

    private isOpen = true;

    get open(): boolean {
        return this.isOpen;
    }

    /** Bytes waiting for this end to read. */
    get depth(): number {
        return this.inbound.depth;
    }

    read(signal?: AbortSignal): Promise<Uint8Array> {
        return this.inbound.pull(signal);
    }

    write(bytes: Uint8Array, signal?: AbortSignal): Promise<void> {
        return this.outbound.push(bytes, signal);
    }

    /**
     * Return this end to a known state — and **only the end this side owns**.
     *
     * `inbound` is what has been delivered *to* this side and nobody read; dropping it is what a
     * host does when it walks away from a transfer, and it is genuine.
     *
     * `outbound` is emphatically **not** cleared, and this used to be the mock's most misleading
     * line. On the host side, `outbound` models bytes already handed to the transport — for WebUSB,
     * submitted `transferOut`s. `reset()` there is `clearHalt`, a `CLEAR_FEATURE(ENDPOINT_HALT)`
     * control request that cancels no transfer and un-queues no byte; the data is the device's and
     * arrives whatever the host does next. Clearing it here made every stray-byte scenario
     * self-healing in tests and self-healing nowhere else: a test could "prove" a fix that was doing
     * nothing, because `withTransferSlot` calls `resetBulk()` on every failure and the strays
     * vanished either way.
     */
    async reset(): Promise<void> {
        this.inbound.clear();
    }

    async close(): Promise<void> {
        this.isOpen = false;
        this.inbound.close();
        this.outbound.close();
    }
}

/**
 * How long the bulk channel must stay quiet before a drain calls it empty, and the ceiling on one
 * drain — the firmware's `DRAIN_QUIET_MS` / `DRAIN_BUDGET_MS`, scaled to a loopback where "quiet"
 * costs a timer turn rather than a USB microframe.
 */
const DRAIN_QUIET_MS = 5;
const DRAIN_BUDGET_MS = 750;

/** Tuning for {@link loopbackLink}. The defaults mirror a high-speed USB interface. */
export interface LoopbackOptions {
    /** Bulk slice size handed to a reader. 512 = a high-speed bulk endpoint's max packet. */
    bulkPacketSize?: number;
    /** Bytes a writer may have outstanding on the bulk channel before it blocks. */
    bulkHighWaterMark?: number;
}

/** The two ends of a link: `host` for the {@link ProtocolClient}, `device` for {@link MockDevice}. */
export interface LoopbackLink {
    host: DeviceLink;
    device: DeviceLink;
    /** Bytes queued but unread on the bulk channel in one direction — the backpressure gauge. */
    bulkDepth(direction: "to-device" | "to-host"): number;
}

export function loopbackLink(options: LoopbackOptions = {}): LoopbackLink {
    const { bulkPacketSize = 512, bulkHighWaterMark = 64 * 1024 } = options;
    // Control frames stay whole (one USB transfer, one frame); bulk is a stream, re-sliced to
    // packet size so the reader must accumulate.
    const hostToDeviceControl = new Channel("message", 64, 16 * 1024);
    const deviceToHostControl = new Channel("message", 64, 16 * 1024);
    const hostToDeviceBulk = new Channel("stream", bulkPacketSize, bulkHighWaterMark);
    const deviceToHostBulk = new Channel("stream", bulkPacketSize, bulkHighWaterMark);

    const host: DeviceLink = {
        control: new LoopbackPipe(deviceToHostControl, hostToDeviceControl),
        bulk: new LoopbackPipe(deviceToHostBulk, hostToDeviceBulk),
        async close() {
            await this.control.close();
            await this.bulk.close();
        },
    };
    const device: DeviceLink = {
        control: new LoopbackPipe(hostToDeviceControl, deviceToHostControl),
        bulk: new LoopbackPipe(hostToDeviceBulk, deviceToHostBulk),
        async close() {
            await this.control.close();
            await this.bulk.close();
        },
    };
    return {
        host,
        device,
        bulkDepth: (direction) => (direction === "to-device" ? hostToDeviceBulk : deviceToHostBulk).depth,
    };
}

// --- the simulated device -----------------------------------------------------

/** One stored object: the bytes the device would have on its card, plus its fingerprint. */
interface Stored {
    bytes: Uint8Array | null;
    crc32: number;
    byteLen: number;
}

/** Reference caps from the spec: 64 routes, 16 trips. */
export const MAX_ROUTES = 64;
export const MAX_TRIPS = 16;

/**
 * The OBCM map-format version the reference firmware's reader reads (`obc_formats::obcm::VERSION`).
 *
 * It lives here, in the **mock device**, and nowhere else in the app: the site's whole §6(c)
 * mechanism is that it asks the connected device rather than assuming, so a constant in the app
 * would be exactly the guess this field exists to remove. A mock device is a device, so it gets to
 * hold one.
 */
export const REFERENCE_OBCM_VERSION = 13;

/** How a {@link MockDevice} starts out. Everything has a working default. */
export interface MockDeviceOptions {
    /** The store epoch to serve. `null` models a device with **no mounted card**, which serves the
     *  2-byte short read and must make a peer fail its ack closed. */
    storeEpoch?: number | null;
    /** The protocol version to serve. Override it to exercise the mismatch path — a peer must
     *  surface and stop, never best-effort decode a wire it does not know. */
    protocolVersion?: number;
    /** The OBCM map-format version this device's firmware reads (§1, #911) — what `OBCC_Spec.md`
     *  §6(c) filters the map catalog on. `null` models a firmware that predates the field and
     *  serves the 6-byte read; a peer must read that as *unknown* and offer the download stating
     *  the version, never as "supports OBCM v0". */
    obcmVersion?: number | null;
    /** The capability word (§1, WX3). `null` models a firmware predating it, which a peer must read
     *  as *unknown* — the old-client path — rather than as a device that announced no features. */
    featureBits?: number | null;
    deviceInfo?: DeviceInfo;
    config?: DeviceConfig;
    /** The update-slot ceiling. An announced `fwImage` past it is rejected before any bytes move. */
    maxFwImageLen?: number;
    maxRoutes?: number;
    maxTrips?: number;
    /** Bulk write size the device uses when streaming a download. */
    chunkSize?: number;
    /**
     * Verify an upload's CRC as bytes arrive and keep **nothing** — what a device with a 300 MB map
     * coming down the cable actually does, since it sinks to the card and has no RAM to buffer in.
     *
     * Off by default because most tests want to assert on {@link MockDevice.stored}; on for the
     * flat-memory measurement, where a device that buffered the object would be the thing consuming
     * the memory rather than the code under test.
     */
    sinkUploads?: boolean;
    /** Free bytes reported by the mounted card; `null` models no readable card. */
    cardFreeBytes?: number | null;
}

/**
 * A device that speaks the protocol over a {@link DeviceLink}.
 *
 * `run()` starts its control loop and resolves when the link closes; nothing else is needed to have
 * a working device on the other end of a `ProtocolClient`.
 */
export class MockDevice {
    private readonly link: DeviceLink;
    private readonly chunkSize: number;
    private readonly maxFwImageLen: number;
    private readonly maxRoutes: number;
    private readonly maxTrips: number;
    private readonly sinkUploads: boolean;
    private readonly cardFreeBytes: number | null;

    private storeEpoch: number | null;
    private readonly protocolVersion: number;
    private readonly obcmVersion: number | null;
    private readonly featureBits: number | null;
    private info: DeviceInfo;
    private config: DeviceConfig;

    private readonly routes = new Map<number, Stored>();
    private readonly rides = new Map<number, Stored>();
    private readonly trips = new Map<number, Stored>();
    /** Maps (`ObjectType.Map`, provisional — see `protocol.ts`). No list object exists for them,
     *  so this is a plain store with no catalog entry beside it. */
    private readonly maps = new Map<number, Stored>();
    /** Staged volume-set shards, keyed by packed `(count,index)`; invisible until a manifest. */
    private readonly mapShards = new Map<number, Stored>();
    private readonly mapSets = new Map<number, Stored>();
    /** The committed sets' terrain shards, by set id — the mock's `MS<id>.OBD`. Lets a test assert
     *  the raster actually landed rather than only that the manifest was accepted. */
    private readonly mapSetTerrain = new Map<number, Stored>();
    /** The staged set's terrain shard (#1044), or `null` when this set carries no raster. A raster
     *  is **not** a shard: it has no index, so it cannot live in `mapShards` without occupying a
     *  slot the manifest never names — the very confusion the separate object type exists to end. */
    private mapTerrain: Stored | null = null;
    /** The shard count every staged `mapShard` has agreed on, or `null` with no set in flight. The
     *  real device holds this in its upload session and refuses a descriptor that contradicts it. */
    private mapShardCount: number | null = null;
    /** The id minted for the set in flight. The device mints it at the **first shard** — it names
     *  the files on the card — and both the raster and the manifest echo it, so the mock must have
     *  one before the manifest commits rather than inventing it there. */
    private mapSetId = 1;
    private readonly routeEntries = new Map<number, RouteListEntry>();
    private readonly rideEntries = new Map<number, RideListEntry>();
    private readonly tripEntries = new Map<number, TripListEntry>();
    /**
     * The monotonic per-ride "a durable copy exists off the device" flag `ackRides` reconciles,
     * with the `synced_at` stamp the real device persists beside it.
     *
     * The stamp is not decoration: it is the anchor auto-expiry (#638) counts from, which is why
     * an ack is a *write* and why the hosted tier must never send one (#894, C5 #904). Modelled
     * here so {@link MockDevice.syncedSidecar} can render the same bytes the firmware writes to
     * `/tracks/SYNCED.SET` — a test that snapshots those bytes either side of a session is
     * checking the thing that would actually change, not a flag it also happens to set.
     */
    private readonly syncedRides = new Map<number, number>();

    private staged: Uint8Array | null = null;
    private nextRouteId = 1;
    private nextRideId = 1;
    private nextTripId = 1;
    private nextMapId = 1;
    private readonly revisions = new Map<number, number>();

    /** The wall clock a `setClock` established, if any — untrusted until then. */
    clock: { utc: number; offsetMin: number } | null = null;
    /** Every command the device has been asked to run, in order. Lets a test assert that the
     *  browser tier never sent an `ackRides` (#894's locked ride-sync semantics). */
    readonly commandLog: number[] = [];
    /** Non-transport failures from detached work — a real defect, not a disconnect. */
    readonly faults: unknown[] = [];

    /** Test/dev-harness visibility into the otherwise invisible staged set. */
    get stagedMapShardCount(): number {
        return this.mapShards.size;
    }

    /** Whether the staged set carries a terrain shard (#1044) — invisible on the wire otherwise. */
    get stagedTerrain(): boolean {
        return this.mapTerrain !== null;
    }

    /** The committed set's terrain shard, or `undefined` when it carried no raster. */
    committedTerrain(setId: number): Stored | undefined {
        return this.mapSetTerrain.get(setId);
    }

    /** Drop everything staged for the set in flight — a commit, an abort, or a torn link.
     *
     *  The id is deliberately **not** advanced here: the device derives set ids from the names on
     *  the card, so an abandoned set frees its id for the next attempt. Only a committed manifest
     *  spends one. */
    private clearStagedSet(): void {
        this.mapShards.clear();
        this.mapTerrain = null;
        this.mapShardCount = null;
    }

    /**
     * The device's commit-time cross-check, as the mock can see it: does this manifest describe the
     * files this session actually staged?
     *
     * The firmware re-reads the manifest, validates it against `OBCA_Spec.md` §5.3 and against the
     * card, and deletes the whole set when it does not match. The mock holds the three parts of
     * that a test can reach without a filesystem: the record count, the terrain record against the
     * raster it received, and each OBCM record's `Bytes` against the shard it holds.
     */
    private manifestDescribesTheStagedSet(manifest: Uint8Array): boolean {
        const parsed = parseSetManifest(manifest);
        if (!parsed) return false;
        const shards = parsed.records.filter((r) => r.role !== SET_ROLE_TERRAIN);
        const terrain = parsed.records.filter((r) => r.role === SET_ROLE_TERRAIN);
        // A terrain record is legal only as the last one, and there is at most one (§5.2).
        if (terrain.length > 1) return false;
        if (terrain.length === 1 && parsed.records[parsed.records.length - 1].role !== SET_ROLE_TERRAIN) return false;
        if (shards.length !== this.mapShards.size) return false;
        // The record the manifest keeps for the raster, against the raster that arrived. This is
        // the check the announce's length rule cannot make: a manifest with N+1 shard records and
        // no terrain record is the *same length* as one with N shards plus terrain.
        const recorded = terrain.length === 1 ? terrain[0].bytes : null;
        const onCard = this.mapTerrain ? this.mapTerrain.byteLen : null;
        if (recorded !== onCard) return false;
        // Each OBCM record against the shard staged at its index (§5.2 derives the filename from
        // the index, so record k is shard k).
        for (let index = 0; index < shards.length; index++) {
            const staged = this.mapShards.get((shards.length << 8) | index);
            if (!staged || staged.byteLen !== shards[index].bytes) return false;
        }
        return true;
    }

    private running = false;

    constructor(link: DeviceLink, options: MockDeviceOptions = {}) {
        this.link = link;
        this.cardFreeBytes = options.cardFreeBytes === undefined ? 8 * 1024 ** 3 : options.cardFreeBytes;
        this.storeEpoch = options.storeEpoch === undefined ? 0xa1b2c3d4 : options.storeEpoch;
        this.protocolVersion = options.protocolVersion ?? PROTOCOL_VERSION;
        this.obcmVersion = options.obcmVersion === undefined ? REFERENCE_OBCM_VERSION : options.obcmVersion;
        // The reference device announces no optional contracts: weather is the phone's path, and a
        // USB host that saw the bit set would learn nothing it could act on. `0` rather than `null`
        // so the loopback still serves the full 11-byte read that a current firmware serves.
        this.featureBits = options.featureBits === undefined ? 0 : options.featureBits;
        this.info = options.deviceInfo ?? {
            firmwareRevision: "0.4.0+abc1234",
            hardwareRevision: "obc-lm20-r1",
            serialNumber: "0011223344556677",
        };
        this.config = options.config ?? { name: "OBC Tourer", units: 0, weatherRefresh: null };
        this.maxFwImageLen = options.maxFwImageLen ?? 1024 * 1024;
        this.maxRoutes = options.maxRoutes ?? MAX_ROUTES;
        this.maxTrips = options.maxTrips ?? MAX_TRIPS;
        this.chunkSize = options.chunkSize ?? 4096;
        this.sinkUploads = options.sinkUploads ?? false;
    }

    // --- seeding ---------------------------------------------------------------

    /** Put a route on the card. `bytes` may be omitted for a catalog entry with no file behind it —
     *  the side-loaded/synthetic case the checked-in `route-list.bin` includes. */
    seedRoute(entry: RouteListEntry, bytes?: Uint8Array): void {
        this.routeEntries.set(entry.objectId, entry);
        this.routes.set(entry.objectId, { bytes: bytes ?? null, crc32: entry.crc32, byteLen: entry.byteLen });
        this.nextRouteId = Math.max(this.nextRouteId, entry.objectId + 1);
    }

    seedRide(entry: RideListEntry, bytes?: Uint8Array): void {
        this.rideEntries.set(entry.objectId, entry);
        this.rides.set(entry.objectId, { bytes: bytes ?? null, crc32: bytes ? Crc32.of(bytes) : 0, byteLen: entry.byteLen });
        this.nextRideId = Math.max(this.nextRideId, entry.objectId + 1);
    }

    seedTrip(entry: TripListEntry, bytes?: Uint8Array): void {
        this.tripEntries.set(entry.objectId, entry);
        this.trips.set(entry.objectId, { bytes: bytes ?? null, crc32: entry.crc32, byteLen: entry.byteLen });
        this.nextTripId = Math.max(this.nextTripId, entry.objectId + 1);
    }

    /**
     * Re-mint the store epoch and empty the card — a chip erase, a factory reset, a reformatted
     * card (spec §1).
     *
     * The whole point is that ids start again from 1, so the *next* ride minted here legitimately
     * carries an id an old ride already used. That is what turns a bare-id library into a silent
     * data loser, and it is why the epoch exists; the iOS companion's `LibraryScopingE2ETests` has
     * the same knob (`setIdentity(epoch:)`) for the same reason. Synced flags go with the card, so
     * they are cleared too.
     */
    reopenIdSpace(storeEpoch: number | null): void {
        this.storeEpoch = storeEpoch;
        this.rides.clear();
        this.rideEntries.clear();
        this.syncedRides.clear();
        this.nextRideId = 1;
    }

    /** The bytes the device holds for an object, or `null` — what a test asserts an upload against. */
    stored(type: ObjectType, objectId: number): Uint8Array | null {
        return this.storeFor(type)?.get(objectId)?.bytes ?? null;
    }

    /** The committed length of a stored object, or `null` if it holds none. The one thing a
     *  {@link MockDeviceOptions.sinkUploads} device can still be asked, since it keeps no bytes. */
    storedLength(type: ObjectType, objectId: number): number | null {
        return this.storeFor(type)?.get(objectId)?.byteLen ?? null;
    }

    /** The staged `/UPDATE.BIN`, if a `fwImage` upload has committed one. */
    get stagedFirmware(): Uint8Array | null {
        return this.staged;
    }

    /** Ride ids the device has flagged synced. Monotonic — nothing ever clears one. */
    get synced(): ReadonlySet<number> {
        return new Set(this.syncedRides.keys());
    }

    /**
     * The `/tracks/SYNCED.SET` sidecar as the firmware would have written it
     * (`obc_app::encode_synced_rides`, v2): `"OBCS"`, version, count, `(id u16, synced_at u32)`
     * entries in insertion order, then a CRC-16/CCITT-FALSE over everything before it.
     *
     * Modelled byte-for-byte so a peer that must not touch the device can be held to it with a
     * byte comparison rather than a flag check — #904's regression pin. The stamp comes from the
     * device's trusted clock, or `0` when no peer has set one, exactly as the firmware does.
     */
    syncedSidecar(): Uint8Array {
        const entries = [...this.syncedRides.entries()];
        const out = new Uint8Array(SYNCED_HEADER_LEN + entries.length * SYNCED_ENTRY_LEN + 2);
        const view = new DataView(out.buffer);
        out.set(SYNCED_MAGIC, 0);
        out[4] = SYNCED_VERSION;
        view.setUint16(6, entries.length, true);
        entries.forEach(([id, syncedAt], i) => {
            const at = SYNCED_HEADER_LEN + i * SYNCED_ENTRY_LEN;
            view.setUint16(at, id, true);
            view.setUint32(at + 2, syncedAt, true);
        });
        view.setUint16(out.length - 2, crc16(out.subarray(0, out.length - 2)), true);
        return out;
    }

    // --- the control loop ------------------------------------------------------

    /**
     * **Nothing reads the bulk channel while no exchange is armed**, and that is the contract the
     * firmware now keeps: an un-armed OUT endpoint NAKs, so bytes a host queued ahead of an announce
     * wait on the wire until a transfer arms and reads them (`usb::data_plane::run`'s idle `select`,
     * which no longer has a read arm at all).
     *
     * This class used to run an `idleDiscard` loop here, mirroring the eager read the firmware had.
     * Both are gone for the same reason: the host pipelines an object's payload behind its announce,
     * so the discard raced the device's own `classify` for the same bytes and small objects lost
     * outright — a 296-byte set manifest was eaten in the field 18 ms before the announce claiming it
     * was answered, and the upload sat at 0% forever. The loopback `Channel` already models the
     * surviving behaviour exactly: an unread write simply queues.
     *
     * What discards unclaimed bytes is therefore the **explicit** drain and nothing else —
     * {@link drainBulk} at the abort handshake, and the firmware's post-answer drain after it refuses
     * an announce whose payload was already in flight.
     */
    /**
     * Empty the bulk channel — the firmware's `drain_bulk_out`, and now the only thing on this
     * device that discards a byte nobody claimed.
     *
     * Correct at exactly two moments, both of which are a termination the host knows about: the
     * abort handshake (before the answer — the peer has stopped and is waiting for one), and a
     * refused announce (*after* the answer — the refusal is what makes the peer stop, and until its
     * queued writes are read they never settle).
     *
     * `quietMs` is how long the channel must stay silent before it counts as empty: `0` for the
     * handshake, where the peer is already quiet, and a beat for the refusal, where it is still
     * unwinding.
     */
    private async drainBulk(quietMs = 0): Promise<void> {
        const deadline = Date.now() + DRAIN_BUDGET_MS;
        for (;;) {
            const stop = new AbortController();
            const timer = setTimeout(() => stop.abort(), quietMs);
            try {
                const stray = await this.link.bulk.read(stop.signal);
                this.strayBytesDiscarded += stray.length;
            } catch {
                return;
            } finally {
                clearTimeout(timer);
            }
            if (Date.now() >= deadline) return;
        }
    }

    /** Serve until the link closes. Rejects only on a defect, never on a normal disconnect. */
    async run(): Promise<void> {
        this.running = true;
        while (this.running) {
            let frame: Uint8Array;
            try {
                frame = await this.link.control.read();
            } catch {
                this.running = false;
                return;
            }
            try {
                await this.handle(decodeFrame(frame));
            } catch (cause) {
                if (cause instanceof PipeError) {
                    this.running = false;
                    return;
                }
                throw cause;
            }
        }
    }

    stop(): void {
        this.running = false;
        // Nothing to release: with the idle read gone, a stopped device holds no reader on the bulk
        // channel. The control loop settles on its next frame or on the pipe closing.
        this.announceGate?.();
        this.announceGate = null;
    }

    private async handle(frame: { selector: number; payload: Uint8Array }): Promise<void> {
        switch (frame.selector) {
            case HostFrame.IdentityRead:
                await this.send(
                    DeviceFrame.Identity,
                    encodeVersionRead({
                        version: this.protocolVersion,
                        storeEpoch: this.storeEpoch,
                        obcmVersion: this.obcmVersion,
                        featureBits: this.featureBits,
                    }),
                );
                return;
            case HostFrame.DeviceInfoRead:
                await this.send(DeviceFrame.DeviceInfo, encodeDeviceInfo(this.info));
                return;
            case HostFrame.ConfigRead:
                await this.send(DeviceFrame.Config, encodeConfig(this.config));
                return;
            case HostFrame.CardFreeRead:
                await this.send(DeviceFrame.CardFree, encodeCardFree(this.cardFreeBytes));
                return;
            case HostFrame.ConfigWrite:
                this.config = decodeConfig(frame.payload);
                return;
            case HostFrame.Command:
                await this.command(frame.payload);
                return;
            case HostFrame.TransferControl:
                await this.transfer(decodeTransferControl(frame.payload));
                return;
            default:
                return; // unknown selector — ignored, as a real device must
        }
    }

    private async send(selector: number, payload?: Uint8Array): Promise<void> {
        await this.link.control.write(encodeFrame(selector, payload));
    }

    private status(msg: StatusMessage): Promise<void> {
        return this.send(DeviceFrame.Status, encodeStatusMessage(msg));
    }

    // --- transfers -------------------------------------------------------------

    /** Non-null while an exchange is armed — the gate that answers a second open with `busy`. */
    private active: { descriptor: TransferControl; abort: AbortController } | null = null;
    /** Unclaimed bulk bytes this device has thrown away — what a test asserts a drain actually did. */
    strayBytesDiscarded = 0;
    /** Resolves the announce {@link holdNextAnnounce} is holding, if one is held. */
    private announceGate: (() => void) | null = null;
    /** The held announce's barrier, taken by the next non-abort descriptor to arrive. */
    private announceHeld: Promise<void> | null = null;

    /**
     * Stall the **next** announce inside the control loop until the returned function is called — a
     * test hook for the one race the device cannot arrange for itself.
     *
     * The firmware classifies a `transferControl` frame behind the shared store lock, which a map
     * render can hold for tens of milliseconds, while the host is already writing that object's
     * payload (it pipelines the two by design, `ProtocolClient.upload`). Holding the announce here
     * reproduces exactly that window: the bytes land on a device with nothing armed, and whether the
     * transfer still completes is the property under test. Blocking the whole control loop is the
     * faithful part — the firmware's control plane is likewise stuck on that lock and answering
     * nothing.
     */
    holdNextAnnounce(): () => void {
        let release!: () => void;
        this.announceHeld = new Promise<void>((resolve) => {
            release = () => {
                this.announceGate = null;
                resolve();
            };
        });
        this.announceGate = release;
        return release;
    }

    private async transfer(d: TransferControl): Promise<void> {
        if (d.op === Op.Abort) {
            const active = this.active;
            if (!active) {
                // The firmware's `TransferDisposition::AnswerIdleAbort`: the peer is about to retry
                // and its queued bytes are still arriving, so empty the pipe before confirming.
                //
                // A post-answer drain still running means the device is not idle yet — on the
                // firmware this request would simply wait its turn at the data plane's `select`.
                await this.draining;
                await this.drainBulk();
                // **Only `mapSet` abandons the staged set** (interface spec §5 rule 6). A
                // `mapShard` or `terrainShard` abort here is the quiesce the host sends after a
                // file the device refused, and the caller is about to re-send that one file —
                // deleting the set under it is how a single CRC refusal used to take the whole map
                // with it. `clearStagedSet` drops the terrain band too, which is exactly why the
                // quiesce must not reach it.
                if (d.type === ObjectType.MapSet) {
                    this.clearStagedSet();
                    await this.status({
                        msg: "transferResult",
                        objectId: d.objectId,
                        status: TransferStatus.Aborted,
                        committedOffset: 0,
                    });
                    return;
                }
                if (d.type === ObjectType.MapShard || d.type === ObjectType.TerrainShard) {
                    // Quiesce only: the pipe is empty, the set — terrain band included — is
                    // untouched.
                    await this.status({
                        msg: "transferResult",
                        objectId: d.objectId,
                        status: TransferStatus.Aborted,
                        committedOffset: 0,
                    });
                    return;
                }
                // Nothing to abort. A real device answers the descriptor rather than staying
                // silent, so a peer that aborts a transfer the device already closed still settles.
                await this.status({
                    msg: "transferResult",
                    objectId: d.objectId,
                    status: TransferStatus.NotFound,
                    committedOffset: 0,
                });
                return;
            }
            active.abort.abort();
            return;
        }
        // A held announce stalls here, in the control loop — see {@link holdNextAnnounce}. Aborts are
        // let through above it, because the frame this models a delay of is the announce.
        const held = this.announceHeld;
        if (held) {
            this.announceHeld = null;
            await held;
        }
        if (this.active) {
            await this.status({
                msg: "transferResult",
                objectId: d.objectId,
                status: TransferStatus.Busy,
                committedOffset: 0,
            });
            return;
        }
        // **A refused announce arms nothing** — §4.2's descriptor-open rejects are decided from the
        // descriptor alone, which is why the firmware answers them on the control plane
        // (`classify_transfer` → `TransferDisposition::Answer`) and never claims the transfer gate.
        // `failNextUpload` still outranks them: it is the hook for the *other* shape, a refusal after
        // the bytes have moved.
        if (d.op === Op.Upload && this.failNextUpload === null) {
            const reject = this.uploadReject(d);
            if (reject !== null) {
                await this.status({ msg: "transferResult", objectId: d.objectId, status: reject, committedOffset: 0 });
                // **Answer, then empty**, and inline in the control loop exactly as the firmware
                // sequences it (`usb::control`'s `drain_after_answer`). The host announced a whole
                // object and has a window of it already submitted; the refusal is what stops it
                // generating more, but those writes settle only when something reads them — an
                // un-armed endpoint just NAKs, so without this a rejected upload deadlocks the
                // sender instead of failing it. Blocking the control loop is the load-bearing half:
                // the next descriptor cannot be served until the pipe is clear, so the drain can
                // never still be running when the retry's payload arrives.
                await this.drainBulk(DRAIN_QUIET_MS);
                return;
            }
        }
        const abort = new AbortController();
        this.active = { descriptor: d, abort };
        // Run the body detached: the control loop has to keep serving while bytes move, which is
        // how an abort reaches a device that is mid-stream. Nothing has to be taken back from an
        // idle reader first: with the eager idle read gone, whatever the host queued ahead of this
        // descriptor is still sitting in the channel, which is precisely the point.
        this.detach(
            (d.op === Op.Upload ? this.receive(d, abort.signal) : this.serve(d, abort.signal))
                .finally(() => {
                    this.active = null;
                })
                .then(async () => {
                    // The post-answer drain, run with the exchange already closed — see `receive`.
                    if (!this.drainAfterAnswer) return;
                    this.drainAfterAnswer = false;
                    // Published while it runs, because the firmware's data plane is **one task**:
                    // its idle-abort drain is only serviced once `run_upload` has returned to the
                    // idle `select`, so an abort can never be answered while a post-answer drain is
                    // still going. Modelling that ordering is what keeps the retry's payload — which
                    // the host sends the moment it sees `aborted` — from landing in this drain.
                    this.draining = this.drainBulk(DRAIN_QUIET_MS).finally(() => {
                        this.draining = null;
                    });
                    await this.draining;
                }),
        );
    }

    /**
     * Refuse the next upload with `status` **after** its bytes have moved — the shape of a torn
     * transfer, as opposed to `uploadReject`'s descriptor-time refusals.
     *
     * A test hook, and the only way to drive the one failure `sendAssembledSetFile` retries without
     * stubbing the client out from under it.
     */
    failNextUploadWith(status: TransferStatus): void {
        this.failNextUpload = status;
    }

    /**
     * Give up on the next upload **partway through its bytes** — the shape of a card that refuses an
     * append, which is the firmware's `run_upload` SD-failure exit and the third of its three drain
     * sites.
     *
     * Distinct from {@link failNextUploadWith}, and the difference is the whole point: a CRC or
     * commit refusal happens *after* the announced length has been consumed, so nothing is left on
     * the wire, whereas this one answers with most of the object still coming. Only the second needs
     * a drain, and without one the sender blocks against an endpoint nobody is reading.
     */
    failNextUploadMidObject(status: TransferStatus): void {
        this.failMidObject = status;
    }

    private failNextUpload: TransferStatus | null = null;
    private failMidObject: TransferStatus | null = null;
    /** Set by a mid-object failure: empty the pipe once the exchange is closed. See {@link receive}. */
    private drainAfterAnswer = false;
    /** A post-answer drain in progress — the device is not idle until it settles. */
    private draining: Promise<void> | null = null;

    private async receive(d: TransferControl, signal: AbortSignal): Promise<void> {
        const midObject = this.failMidObject;
        if (midObject !== null) {
            this.failMidObject = null;
            // Take a slice and stop, exactly as the firmware does when `stage.push` returns false.
            try {
                await this.link.bulk.read(signal);
            } catch {
                // The peer gave up first; the answer below is still the honest one.
            }
            await this.status({ msg: "transferResult", objectId: d.objectId, status: midObject, committedOffset: 0 });
            // Requested rather than run here, because the firmware's `close_transfer` releases the
            // gate *before* this answer: by the time it drains, the exchange is closed, so a quiesce
            // abort arriving during the drain finds nothing armed and is answered. Draining while
            // this runner still counted as active would swallow that abort instead.
            this.drainAfterAnswer = true;
            return;
        }
        const forced = this.failNextUpload;
        if (forced !== null) {
            this.failNextUpload = null;
            // Drain the object first: a real device consumes what it was announced before it
            // verifies, and the host must not be left blocked on an endpoint nobody is reading.
            let drained = 0;
            try {
                while (drained < d.totalLen) drained += (await this.link.bulk.read(signal)).length;
            } catch {
                // The peer gave up first; the answer below is still the honest one.
            }
            await this.status({ msg: "transferResult", objectId: d.objectId, status: forced, committedOffset: 0 });
            return;
        }
        // The descriptor-time rejects are not decided here — see {@link transfer}, which refuses them
        // on the control plane without arming anything, as `classify_transfer` does.
        // **A re-send destroys what it re-sends, at its first byte.** The device streams a shard or
        // a raster straight into its final name with `ReadWriteCreateOrTruncate`, so the moment a
        // re-send starts, the file that was under that name is gone — and if the re-send then fails,
        // the set is one file short. A mock that only added files on success could never reach that
        // state, and the firmware bug it hides (a session still counting a file the card no longer
        // holds, so the manifest passes its announce and dies at the set-deleting commit) is exactly
        // what #1044's last review round found. Un-stage first, re-add at commit.
        if (d.type === ObjectType.MapShard) this.mapShards.delete(d.objectId);
        if (d.type === ObjectType.TerrainShard) this.mapTerrain = null;

        // A sinking device holds nothing: the real one writes each slice to the card and keeps only
        // the running CRC, which is the *only* way a 300 MB map fits on a microcontroller at all.
        const buffer = this.sinkUploads ? null : new Uint8Array(d.totalLen);
        const crc = new Crc32();
        let got = 0;
        try {
            while (got < d.totalLen) {
                const slice = await this.link.bulk.read(signal);
                const take = Math.min(slice.length, d.totalLen - got);
                buffer?.set(slice.subarray(0, take), got);
                crc.update(slice.subarray(0, take));
                got += take;
            }
        } catch {
            // A cancelled or dropped upload is discarded whole — transfers restart, never resume.
            //
            // **Drain before answering**, exactly as `run_upload`'s abort arm does: a cancel leaves
            // the rest of the announced object still arriving, and this is the one moment the peer is
            // provably waiting rather than pumping. Nothing else will take those bytes — an un-armed
            // endpoint NAKs rather than discarding — so skipping it hands them to the retry as its
            // opening payload and fails a whole-object CRC for reasons the exchange cannot explain.
            await this.drainBulk(DRAIN_QUIET_MS);
            await this.status({
                msg: "transferResult",
                objectId: d.objectId,
                status: TransferStatus.Aborted,
                committedOffset: 0,
            });
            return;
        }

        if (crc.value() !== d.crc32) {
            await this.status({
                msg: "transferResult",
                objectId: d.objectId,
                status: TransferStatus.CrcMismatch,
                committedOffset: 0,
            });
            return;
        }
        // The device's commit is not a formality — it validates the bytes it just took and can
        // still refuse them (a raster that is not an OBCT, a manifest that does not describe the
        // files beside it). `null` is that refusal, and it must reach the host as a status rather
        // than as a silently-accepted object, or a whole class of firmware behaviour goes untested.
        const objectId = this.commit(d, buffer, got);
        await this.status({
            msg: "transferResult",
            objectId: objectId ?? d.objectId,
            status: objectId === null ? TransferStatus.Error : TransferStatus.Committed,
            committedOffset: objectId === null ? 0 : d.totalLen,
        });
    }

    /** The descriptor-open rejects, all decided *before* a byte is consumed (§4.2). */
    private uploadReject(d: TransferControl): TransferStatus | null {
        if (d.type === ObjectType.FwImage) {
            return d.totalLen > this.maxFwImageLen ? TransferStatus.Error : null;
        }
        // ---- volume sets: the rules that live *between* transfers (spec §4.1, OBCA §5.4) ----
        //
        // These were once "accept anything with a plausible shape", and that hole is what let #1044
        // ship: a host that skipped the terrain shard and a device that counted records disagreed
        // about the manifest's length by exactly 56 bytes, and no test in this repo could see it
        // because the mock had no length rule to break. What the mock enforces is now what the
        // firmware enforces, expressed the same way — count, order, and the manifest's exact length.
        if (d.type === ObjectType.MapShard) {
            const count = d.objectId >>> 8;
            const index = d.objectId & 0xff;
            if (!(count >= 1 && count <= 32 && index < count)) return TransferStatus.NotFound;
            // Every shard restates the set's shape; one that contradicts the set in flight is a
            // mismatch, not a second set silently merged into the first.
            if (this.mapShardCount !== null && this.mapShardCount !== count) return TransferStatus.Error;
            return null;
        }
        if (d.type === ObjectType.TerrainShard) {
            // A malformed id is answered before anything about the session, as it is for a shard.
            if (d.objectId !== NEW_OBJECT_ID) return TransferStatus.NotFound;
            // A raster names no set of its own — the set id is minted by the first shard.
            if (this.mapShardCount === null) return TransferStatus.Error;
            // OBCA §5.2 caps a manifest at 32 records, so a full set has no room for a terrain one.
            if (this.mapShardCount + 1 > 32) return TransferStatus.StorageFull;
            // …and it has to be long enough to be an OBCT at all (map rule 3, against OBCT).
            if (d.totalLen < OBCT_HEADER_LEN) return TransferStatus.Error;
            return null;
        }
        if (d.type === ObjectType.MapSet) {
            if (d.objectId !== NEW_OBJECT_ID) return TransferStatus.NotFound;
            const count = this.mapShardCount;
            if (count === null) return TransferStatus.Error;
            // Manifest-last: every shard the manifest will name must already have committed.
            if (this.mapShards.size !== count) return TransferStatus.Error;
            // …and its announced length is fixed by the *record* count — shards plus the raster.
            if (d.totalLen !== manifestLen(count + (this.mapTerrain ? 1 : 0))) return TransferStatus.Error;
            return null;
        }
        const store = this.storeFor(d.type);
        if (!store) return TransferStatus.NotFound;
        const known = store.has(d.objectId);
        // A replace-by-id reuses a slot rather than growing the catalog, so it is exempt from the
        // cap even when the catalog is full — updating the route you are navigating must not fail.
        if (known) return null;
        const cap = d.type === ObjectType.Route ? this.maxRoutes : d.type === ObjectType.Trip ? this.maxTrips : Infinity;
        if (store.size >= cap) return TransferStatus.StorageFull;
        return d.objectId === NEW_OBJECT_ID ? null : TransferStatus.NotFound;
    }

    /** The assigned object id, or `null` when the commit itself refuses the bytes it just took. */
    private commit(d: TransferControl, bytes: Uint8Array | null, byteLen: number): number | null {
        if (d.type === ObjectType.FwImage) {
            // A CRC-verified commit promotes the staged bytes over any existing UPDATE.BIN, and
            // the singleton slot means the result echoes id 0 rather than assigning one.
            this.staged = bytes;
            return SINGLETON_OBJECT_ID;
        }
        if (d.type === ObjectType.MapShard) {
            // The set id is minted by the **first** shard, as the device's is: it is what a raster
            // and the manifest both echo, and what a set is called on the card.
            if (this.mapShardCount === null) this.mapSetId = this.nextMapId;
            this.mapShardCount = d.objectId >>> 8;
            this.mapShards.set(d.objectId, { bytes, crc32: d.crc32, byteLen });
            return d.objectId;
        }
        if (d.type === ObjectType.TerrainShard) {
            // The device patches the held-back magic in only after the OBCT header prefix
            // validates, and deletes the file and answers `error` when it does not.
            if (bytes && !isObct(bytes)) {
                this.mapTerrain = null;
                return null;
            }
            // A re-sent raster that lands overwrites the one file; it is never a second record.
            this.mapTerrain = { bytes, crc32: d.crc32, byteLen };
            // The result echoes the **set id**, as the manifest's does — a raster has no part to
            // correlate against, and the set id is the only identity it has.
            return this.mapSetId;
        }
        if (d.type === ObjectType.MapSet) {
            // **The commit-time cross-check** (spec §4.1 rule 7): the manifest is re-read and
            // checked against the files actually staged, and a manifest that does not describe them
            // is refused with the whole set deleted. The announce's length rule cannot see any of
            // this — a same-length impostor passes it — so modelling it here is what gives that
            // firmware path its only coverage.
            if (bytes && !this.manifestDescribesTheStagedSet(bytes)) {
                this.clearStagedSet();
                return null;
            }
            const id = this.mapSetId;
            this.nextMapId = id + 1;
            this.mapSets.set(id, { bytes, crc32: d.crc32, byteLen });
            if (this.mapTerrain) this.mapSetTerrain.set(id, this.mapTerrain);
            this.clearStagedSet();
            return id;
        }
        const store = this.storeFor(d.type);
        if (!store) return d.objectId;

        if (d.objectId === NEW_OBJECT_ID) {
            // Fresh-upload dedup: identical content already stored answers with the *existing* id
            // and stores nothing, so a retry after a lost ack converges instead of minting a twin.
            for (const [id, stored] of store) {
                if (stored.crc32 === d.crc32 && stored.byteLen === byteLen) return id;
            }
        }
        const id = d.objectId === NEW_OBJECT_ID ? this.mintId(d.type) : d.objectId;
        store.set(id, { bytes, crc32: d.crc32, byteLen });
        if (d.type === ObjectType.Route && !this.routeEntries.has(id)) {
            // The device would read these from the stored OBCR header; this mock does not parse
            // OBCR, so an uploaded route lists with a generated name and zeroed metrics.
            this.routeEntries.set(id, {
                objectId: id,
                byteLen,
                distanceM: 0,
                ascentM: 0,
                pointCount: 0,
                waypointCount: 0,
                name: `Route ${id}`,
                crc32: d.crc32,
                expiresAt: 0,
                retention: 0,
            });
        }
        const entry = this.routeEntries.get(id);
        if (d.type === ObjectType.Route && entry) {
            this.routeEntries.set(id, { ...entry, byteLen, crc32: d.crc32 });
        }
        if (d.type === ObjectType.Trip) {
            const existing = this.tripEntries.get(id);
            this.tripEntries.set(id, {
                objectId: id,
                byteLen,
                totalDistanceM: existing?.totalDistanceM ?? 0,
                totalAscentM: existing?.totalAscentM ?? 0,
                stageCount: existing?.stageCount ?? 0,
                name: existing?.name ?? `Trip ${id}`,
                crc32: d.crc32,
            });
        }
        this.bumpStore(d.type);
        return id;
    }

    private mintId(type: ObjectType): number {
        if (type === ObjectType.Route) return this.nextRouteId++;
        if (type === ObjectType.Trip) return this.nextTripId++;
        if (type === ObjectType.Map) return this.nextMapId++;
        return this.nextRideId++;
    }

    private async serve(d: TransferControl, signal: AbortSignal): Promise<void> {
        const bytes = this.objectBytes(d.type, d.objectId);
        if (!bytes) {
            await this.status({
                msg: "transferResult",
                objectId: d.objectId,
                status: TransferStatus.NotFound,
                committedOffset: 0,
            });
            return;
        }
        const crc32 = Crc32.of(bytes);
        await this.status({
            msg: "downloadAnnounce",
            descriptor: { op: Op.Download, type: d.type, objectId: d.objectId, totalLen: bytes.length, crc32 },
        });
        try {
            for (let at = 0; at < bytes.length; at += this.chunkSize) {
                // The signal has to reach the *write*, not only the loop: a device parked on a
                // full endpoint would otherwise keep the host waiting for its `aborted` result
                // until something else broke the deadlock.
                await this.link.bulk.write(bytes.subarray(at, Math.min(at + this.chunkSize, bytes.length)), signal);
            }
        } catch {
            await this.status({
                msg: "transferResult",
                objectId: d.objectId,
                status: TransferStatus.Aborted,
                committedOffset: 0,
            });
            return;
        }
        await this.status({
            msg: "transferResult",
            objectId: d.objectId,
            status: TransferStatus.Committed,
            committedOffset: bytes.length,
        });
    }

    /** The bytes behind a download request: a stored object, or a freshly-encoded list object. */
    private objectBytes(type: ObjectType, objectId: number): Uint8Array | null {
        switch (type) {
            case ObjectType.RouteList:
                return encodeList(this.routeEntries, ROUTE_ENTRY_LEN, encodeRouteListEntry);
            case ObjectType.RideList:
                return encodeList(this.rideEntries, RIDE_ENTRY_LEN, encodeRideListEntry);
            case ObjectType.TripList:
                return encodeList(this.tripEntries, TRIP_ENTRY_LEN, encodeTripListEntry);
            default:
                return this.storeFor(type)?.get(objectId)?.bytes ?? null;
        }
    }

    private storeFor(type: ObjectType): Map<number, Stored> | null {
        if (type === ObjectType.Route) return this.routes;
        if (type === ObjectType.Ride) return this.rides;
        if (type === ObjectType.Trip) return this.trips;
        if (type === ObjectType.Map) return this.maps;
        return null;
    }

    private bumpStore(type: ObjectType): void {
        const revision = (this.revisions.get(type) ?? 0) + 1;
        this.revisions.set(type, revision);
        this.detach(this.status({ msg: "storeChanged", type, revision }));
    }

    /**
     * Let a detached task finish without turning a normal disconnect into an unhandled rejection.
     *
     * A device that is mid-stream when the host closes the link will fail its next write, and that
     * is the ordinary end of a session, not a defect. Anything that is *not* a transport failure is
     * kept in {@link faults} so a test can find a real bug rather than have it swallowed.
     */
    private detach(task: Promise<unknown>): void {
        void task.catch((cause: unknown) => {
            if (!(cause instanceof PipeError)) this.faults.push(cause);
        });
    }

    // --- commands --------------------------------------------------------------

    private async command(bytes: Uint8Array): Promise<void> {
        if (bytes.length < 1) return;
        const cmd = bytes[0];
        this.commandLog.push(cmd);
        const [status, detail] = this.runCommand(cmd, bytes);
        await this.status({ msg: "commandResult", command: cmd, status, detail });
    }

    private runCommand(cmd: number, bytes: Uint8Array): [CommandStatus, number] {
        switch (cmd) {
            case Command.DeleteObject: {
                if (bytes.length !== 4) return [CommandStatus.Error, 0];
                const type = bytes[1];
                const id = viewOf(bytes).getUint16(2, true);
                // Ride deletion over the link is reserved: rides are deleted on the device itself.
                if (type === ObjectType.Ride) return [CommandStatus.NotFound, 0];
                const store = this.storeFor(type as ObjectType);
                if (!store?.delete(id)) return [CommandStatus.NotFound, 0];
                (type === ObjectType.Route ? this.routeEntries : this.tripEntries).delete(id);
                this.bumpStore(type as ObjectType);
                return [CommandStatus.Ok, 0];
            }
            case Command.AckRides: {
                if (bytes.length < 2) return [CommandStatus.Error, 0];
                const count = bytes[1];
                if (bytes.length < 2 + count * 2) return [CommandStatus.Error, 0];
                const view = viewOf(bytes);
                let flagged = 0;
                for (let i = 0; i < count; i++) {
                    const id = view.getUint16(2 + i * 2, true);
                    // Unknown ids are ignored, not an error: the peer may hold rides the device has
                    // since deleted. Monotonic — a flag is never cleared, and an already-synced ride
                    // keeps its **original** stamp (sync time is first-sync, not last), because that
                    // stamp is what the expiry countdown is anchored to.
                    if (this.rides.has(id) && !this.syncedRides.has(id)) {
                        this.syncedRides.set(id, this.clock?.utc ?? 0);
                        flagged++;
                    }
                }
                if (flagged) void this.bumpStore(ObjectType.Ride);
                return [CommandStatus.Ok, Math.min(flagged, 255)];
            }
            case Command.InstallFw:
                // Precedence busy > noStaged > invalid > ok; this device is never busy and never
                // runs the multi-second scan inside the handler, so it accepts and lets the
                // on-glass confirm surface a bad image.
                return this.staged ? [CommandStatus.Ok, 0] : [CommandStatus.NotFound, 0];
            case Command.ForgetBond:
                return [CommandStatus.Ok, 0];
            case Command.SetClock: {
                if (bytes.length !== 7) return [CommandStatus.Error, 0];
                const view = viewOf(bytes);
                const utc = view.getUint32(1, true);
                const offsetMin = view.getInt16(5, true);
                if (utc < SET_CLOCK_MIN_UTC || Math.abs(offsetMin) > SET_CLOCK_MAX_OFFSET_MIN) {
                    return [CommandStatus.Error, 0];
                }
                this.clock = { utc, offsetMin };
                return [CommandStatus.Ok, 0];
            }
            case Command.SetRouteRetention: {
                if (bytes.length !== 4) return [CommandStatus.Error, 0];
                const id = viewOf(bytes).getUint16(1, true);
                const retention = bytes[3];
                if (retention > MAX_RETENTION) return [CommandStatus.Error, 0];
                const entry = this.routeEntries.get(id);
                if (!entry) return [CommandStatus.NotFound, 0];
                // Only a real change moves the store; setting the level a route already has is ok
                // with no revision bump.
                if (entry.retention !== retention) {
                    this.routeEntries.set(id, { ...entry, retention });
                    this.bumpStore(ObjectType.Route);
                }
                return [CommandStatus.Ok, 0];
            }
            default:
                return [CommandStatus.UnknownCommand, 0];
        }
    }
}

/**
 * A client wired to a running {@link MockDevice} — the one-liner C4, C5 and every test start from.
 *
 * Seed the device, drive the client, `close()` when done. The device's control loop runs detached;
 * closing the client closes the link, which ends it.
 */
export function loopbackDevice(
    options: LoopbackOptions & MockDeviceOptions = {},
): { client: ProtocolClient; device: MockDevice; link: LoopbackLink; close: () => Promise<void> } {
    const link = loopbackLink(options);
    const device = new MockDevice(link.device, options);
    void device.run();
    const client = new ProtocolClient(link.host);
    return {
        client,
        device,
        link,
        close: async () => {
            device.stop();
            await client.close();
            await link.device.close();
        },
    };
}

// --- the two card formats the mock has to judge, not just store -----------------
//
// A device's *commit* is where a volume set stops being bytes and becomes a map, and it is where
// the firmware does its only real parsing: the raster must open as an OBCT container, and the
// manifest must describe the files beside it. Neither is visible to the announce rules above — a
// same-length impostor manifest passes every one of them — so a mock that only stored what it was
// handed left that whole firmware path untested. These are the smallest readers that let it judge.

/** `OBCA_Spec.md` §5.2's `Role == 3`: the set's terrain record, and always the last one. */
const SET_ROLE_TERRAIN = 3;

/** One record of a parsed OBCS manifest — the two fields a cross-check needs. */
interface SetManifestRecord {
    readonly role: number;
    readonly bytes: number;
}

/**
 * Parse an OBCS set manifest (`OBCA_Spec.md` §5.2), or `null` when it is not one.
 *
 * Deliberately partial: magic, version, `Shard Count`, the exact length that count fixes, and each
 * record's role + `Bytes`. Everything else (bboxes, the set id, digests) is checked by the device
 * against files this mock does not model, and inventing checks it cannot really make would be worse
 * than having none.
 */
function parseSetManifest(bytes: Uint8Array): { readonly records: SetManifestRecord[] } | null {
    if (bytes.length < OBCS_HEADER_LEN) return null;
    if (String.fromCharCode(...bytes.subarray(0, 4)) !== "OBCS") return null;
    if (bytes[4] !== OBCS_VERSION) return null;
    const count = bytes[6];
    if (count < 1 || count > 32) return null;
    if (bytes.length !== manifestLen(count)) return null;
    if (bytes[7] >= count) return null; // Core Shard is an index into the records
    const view = viewOf(bytes);
    const records: SetManifestRecord[] = [];
    for (let i = 0; i < count; i++) {
        const at = OBCS_HEADER_LEN + i * OBCS_RECORD_LEN;
        records.push({ role: bytes[at], bytes: view.getUint32(at + 20, true) });
    }
    return { records };
}

/** Whether these bytes open as an OBCT terrain container this firmware reads (`OBCT_Spec.md` §4). */
function isObct(bytes: Uint8Array): boolean {
    if (bytes.length < OBCT_HEADER_LEN) return false;
    return String.fromCharCode(...bytes.subarray(0, 4)) === OBCT_MAGIC && bytes[4] === OBCT_VERSION;
}

// --- the synced-ride sidecar --------------------------------------------------
//
// `obc-app/src/ride.rs`'s v2 layout, restated here so {@link MockDevice.syncedSidecar} can produce
// the bytes the firmware would. Not a wire format — it is a file on the card — but it is the file
// an `ackRides` mutates, so it is the right thing for a "this peer changed nothing" test to compare.

const SYNCED_MAGIC = new Uint8Array([0x4f, 0x42, 0x43, 0x53]); // "OBCS"
const SYNCED_VERSION = 2;
const SYNCED_HEADER_LEN = 8;
const SYNCED_ENTRY_LEN = 6;

/** CRC-16/CCITT-FALSE, the sidecar's tail check (`obc_app::store_meta::crc16`). */
function crc16(data: Uint8Array): number {
    let crc = 0xffff;
    for (const byte of data) {
        crc ^= byte << 8;
        for (let bit = 0; bit < 8; bit++) crc = crc & 0x8000 ? ((crc << 1) ^ 0x1021) & 0xffff : (crc << 1) & 0xffff;
    }
    return crc;
}

/** Build a list object from a catalog: the 6-byte header plus fixed entries in id order. */
function encodeList<T>(
    entries: Map<number, T>,
    entryLen: number,
    encodeEntry: (entry: T) => Uint8Array,
): Uint8Array {
    const sorted = [...entries.entries()].sort(([a], [b]) => a - b).map(([, e]) => e);
    const out = new Uint8Array(LIST_HEADER_LEN + sorted.length * entryLen);
    out.set(encodeListHeader({ count: sorted.length, total: sorted.length, entryLen }), 0);
    sorted.forEach((entry, i) => out.set(encodeEntry(entry), LIST_HEADER_LEN + i * entryLen));
    return out;
}
