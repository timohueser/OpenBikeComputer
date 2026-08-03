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
 * and a client that fires writes without awaiting them would outrun a device whose SD card tops out
 * in the high hundreds of KB/s. Faking it here is what makes that bug fail in CI.
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

    /** Clear **both** directions: an abandoned exchange leaves stale bytes at both ends, and the
     *  device discarding its partial is half of what makes the channel reusable. */
    async reset(): Promise<void> {
        this.inbound.clear();
        this.outbound.clear();
    }

    async close(): Promise<void> {
        this.isOpen = false;
        this.inbound.close();
        this.outbound.close();
    }
}

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

    private running = false;

    constructor(link: DeviceLink, options: MockDeviceOptions = {}) {
        this.link = link;
        this.cardFreeBytes = options.cardFreeBytes === undefined ? 8 * 1024 ** 3 : options.cardFreeBytes;
        this.storeEpoch = options.storeEpoch === undefined ? 0xa1b2c3d4 : options.storeEpoch;
        this.protocolVersion = options.protocolVersion ?? PROTOCOL_VERSION;
        this.obcmVersion = options.obcmVersion === undefined ? REFERENCE_OBCM_VERSION : options.obcmVersion;
        this.info = options.deviceInfo ?? {
            firmwareRevision: "0.4.0+abc1234",
            hardwareRevision: "obc-lm20-r1",
            serialNumber: "0011223344556677",
        };
        this.config = options.config ?? { name: "OBC Tourer", units: 0 };
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

    private async transfer(d: TransferControl): Promise<void> {
        if (d.op === Op.Abort) {
            const active = this.active;
            if (!active) {
                if (d.type === ObjectType.MapShard || d.type === ObjectType.MapSet) {
                    this.mapShards.clear();
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
        if (this.active) {
            await this.status({
                msg: "transferResult",
                objectId: d.objectId,
                status: TransferStatus.Busy,
                committedOffset: 0,
            });
            return;
        }
        const abort = new AbortController();
        this.active = { descriptor: d, abort };
        // Run the body detached: the control loop has to keep serving while bytes move, which is
        // how an abort reaches a device that is mid-stream.
        this.detach(
            (d.op === Op.Upload ? this.receive(d, abort.signal) : this.serve(d, abort.signal)).finally(() => {
                this.active = null;
            }),
        );
    }

    private async receive(d: TransferControl, signal: AbortSignal): Promise<void> {
        const reject = this.uploadReject(d);
        if (reject !== null) {
            await this.status({ msg: "transferResult", objectId: d.objectId, status: reject, committedOffset: 0 });
            return;
        }

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
        const objectId = this.commit(d, buffer, got);
        await this.status({
            msg: "transferResult",
            objectId,
            status: TransferStatus.Committed,
            committedOffset: d.totalLen,
        });
    }

    /** The descriptor-open rejects, all decided *before* a byte is consumed (§4.2). */
    private uploadReject(d: TransferControl): TransferStatus | null {
        if (d.type === ObjectType.FwImage) {
            return d.totalLen > this.maxFwImageLen ? TransferStatus.Error : null;
        }
        if (d.type === ObjectType.MapShard) {
            const count = d.objectId >>> 8;
            const index = d.objectId & 0xff;
            return count >= 1 && count <= 32 && index < count ? null : TransferStatus.NotFound;
        }
        if (d.type === ObjectType.MapSet) {
            return d.objectId === NEW_OBJECT_ID && this.mapShards.size > 0 ? null : TransferStatus.NotFound;
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

    private commit(d: TransferControl, bytes: Uint8Array | null, byteLen: number): number {
        if (d.type === ObjectType.FwImage) {
            // A CRC-verified commit promotes the staged bytes over any existing UPDATE.BIN, and
            // the singleton slot means the result echoes id 0 rather than assigning one.
            this.staged = bytes;
            return SINGLETON_OBJECT_ID;
        }
        if (d.type === ObjectType.MapShard) {
            this.mapShards.set(d.objectId, { bytes, crc32: d.crc32, byteLen });
            return d.objectId;
        }
        if (d.type === ObjectType.MapSet) {
            const id = this.nextMapId++;
            this.mapSets.set(id, { bytes, crc32: d.crc32, byteLen });
            this.mapShards.clear();
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
