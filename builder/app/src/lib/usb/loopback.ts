/**
 * An in-memory device: two record channels wired to a simulated flat store, speaking protocol v4.
 *
 * **This is a deliverable, not a test helper.** The whole browser and desktop stack is built and
 * tested against it, so it has to behave like a device rather than like an echo: id assignment, the
 * `(ObjectId, Revision)` cursor and its staleness check, the compare-and-swap on a replace, retained
 * revisions, `busy` on a second transfer, the kinds a client may not write, `noSpace` with the bytes
 * required, and the bilateral cancel are all here, because those are the paths a UI gets wrong and
 * otherwise only discovers on hardware.
 *
 * Three things it deliberately does **not** do, each stated rather than quietly missing:
 *
 * - **Parse payloads.** An object crosses the wire as opaque bytes the store writes verbatim, so an
 *   uploaded route lists with the display name the `PUT` carried and nothing else. That is exactly
 *   what a v4 `LIST` entry holds, which is the point: there is no richer catalog to model.
 * - **Model USB itself.** Timing, stalls and enumeration belong to the WebUSB pipe. What this models
 *   is the protocol, plus the one transport property a mock usually fakes away and shouldn't: both
 *   channels hand bytes over in packet-sized slices, so a client that assumed a read returns a whole
 *   record fails here rather than on a rider's desk.
 * - **Arm an update.** {@link MockDevice} answers `ARM` with `rejected` by default, because that is
 *   what the device's current policy does (§4's dev-window gap). {@link MockDeviceOptions.armPolicy}
 *   opens it so the success shape has a test, and the default stays the truth.
 */

import { FlatStoreClient } from "./client";
import { Crc32 } from "./crc32";
import { PipeError, throwIfAborted, type BytePipe, type DeviceLink } from "./pipe";
import {
    MAX_DEVICE_RECORD,
    MAX_HOST_CONTROL_RECORD,
    MAX_HOST_STREAM_RECORD,
    MAX_STREAM_PAYLOAD,
    RecordChannel,
    encodeDeviceInfo,
    type DeviceInfo,
} from "./records";
import {
    Detail,
    EntryFlags,
    ErrorCode,
    LIST_ENTRY_LEN,
    LIST_PREFIX_LEN,
    HEADER_LEN,
    NO_OBJECT,
    ObjectKind,
    ObjectState,
    Opcode,
    decodeRequest,
    encodeArmResponse,
    encodeCancelResponse,
    encodeErrorResponse,
    encodeFormatResponse,
    encodeGetResponse,
    encodeListResponse,
    encodePutResponse,
    encodeRemoveResponse,
    encodeStatusResponse,
    encodeStreamRecord,
    isFailure,
    refusal,
    splitStreamRecord,
    toSafeNumber,
    type CatalogEntry,
    type ObjectRef,
    type PutRequest,
    type Refusal,
    type Request,
} from "./protocol";

// --- the pipe ----------------------------------------------------------------

/**
 * One direction of the loopback.
 *
 * **Backpressure** is a byte high-water mark: a writer that has filled the channel waits for the
 * reader to drain it. Real backpressure comes from the device NAKing an endpoint it hasn't drained,
 * and a client that queued writes without ever retiring them would outrun any real device. Faking it
 * here is what makes that bug fail in CI.
 *
 * **Segmentation**: every write is re-sliced to `packetSize`, on *both* channels. Under §5.2 a
 * record may span packets on either pair, so a channel that kept writes whole would be modelling a
 * transport property USB does not have — and would hide precisely the reassembly bug the v1
 * envelope's one-frame-per-transfer rule used to make impossible.
 */
class Channel {
    private readonly chunks: Uint8Array[] = [];
    private queued = 0;
    private readers: Array<{ resolve: (v: Uint8Array) => void; reject: (e: unknown) => void }> = [];
    private writers: Array<() => void> = [];
    private closed = false;

    constructor(
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
        for (let at = 0; at < owned.length; at += this.packetSize) {
            this.enqueue(owned.subarray(at, Math.min(at + this.packetSize, owned.length)));
        }
        // A writer parked on the high-water mark has to stay cancellable: a cancelled upload whose
        // last write is waiting for room would otherwise never observe the abort, and the caller
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
     * `outbound` is emphatically **not** cleared. On the host side it models bytes already handed to
     * the transport — submitted `transferOut`s — and `reset()` there is `clearHalt`, which cancels no
     * transfer and un-queues no byte. Clearing it here would make every stray-byte scenario
     * self-healing in tests and self-healing nowhere else.
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

/** Tuning for {@link loopbackLink}. The defaults mirror a high-speed USB interface. */
export interface LoopbackOptions {
    /** Slice size handed to a reader on both channels. 512 = a high-speed bulk endpoint's max packet. */
    packetSize?: number;
    /** Bytes a writer may have outstanding on the stream channel before it blocks. */
    streamHighWaterMark?: number;
}

/** The two ends of a link: `host` for the client, `device` for {@link MockDevice}. */
export interface LoopbackLink {
    host: DeviceLink;
    device: DeviceLink;
    /** Bytes queued but unread on the stream channel in one direction — the backpressure gauge. */
    streamDepth(direction: "to-device" | "to-host"): number;
}

/**
 * Two record channels and the EP0 read beside them.
 *
 * `vendorIn` is on the **host** link only, because that is the direction §5.2.1 defines: the host
 * asks and the device answers. The device end has no such method and needs none.
 */
export function loopbackLink(options: LoopbackOptions & { deviceInfo?: DeviceInfo } = {}): LoopbackLink {
    const { packetSize = 512, streamHighWaterMark = 64 * 1024 } = options;
    const hostToDeviceControl = new Channel(packetSize, 16 * 1024);
    const deviceToHostControl = new Channel(packetSize, 16 * 1024);
    const hostToDeviceStream = new Channel(packetSize, streamHighWaterMark);
    const deviceToHostStream = new Channel(packetSize, streamHighWaterMark);
    const info = options.deviceInfo ?? DEFAULT_DEVICE_INFO;

    const host: DeviceLink = {
        control: new LoopbackPipe(deviceToHostControl, hostToDeviceControl),
        stream: new LoopbackPipe(deviceToHostStream, hostToDeviceStream),
        async vendorIn(request: number, _value: number, length: number) {
            // Modelled to the letter of §5.2.1, short transfer included: a host that assumed it got
            // `length` bytes back would work here and fail on glass.
            if (request !== 0x20) throw new PipeError("device-error", `the device stalled vendor request ${request}.`);
            return encodeDeviceInfo(info).subarray(0, length);
        },
        async close() {
            await this.control.close();
            await this.stream.close();
        },
    };
    const device: DeviceLink = {
        control: new LoopbackPipe(hostToDeviceControl, deviceToHostControl),
        stream: new LoopbackPipe(hostToDeviceStream, deviceToHostStream),
        async close() {
            await this.control.close();
            await this.stream.close();
        },
    };
    return {
        host,
        device,
        streamDepth: (direction) => (direction === "to-device" ? hostToDeviceStream : deviceToHostStream).depth,
    };
}

// --- the simulated store -------------------------------------------------------

/** One catalog entry and the payload behind it. `bytes` is null for a seeded metadata-only row. */
interface Stored {
    meta: CatalogEntry;
    bytes: Uint8Array | null;
}

/** The page size §5.2's 4,112-byte device→host ceiling allows: 46 entries. */
export const LIST_PAGE_ENTRIES = Math.floor((MAX_DEVICE_RECORD - HEADER_LEN - LIST_PREFIX_LEN) / LIST_ENTRY_LEN);

const DEFAULT_DEVICE_INFO: DeviceInfo = {
    firmwareRevision: "0.4.0+abc1234",
    hardwareRevision: "obc-lm20-r1",
    serialNumber: "0011223344556677",
};

/** `FLAT_Store_Format.md` §5.7's store identity, so a fixture-driven test has a real one to name. */
export const REFERENCE_STORE_ID = "8f2c41d96b074ea3b1559c207de83466";

/** How a {@link MockDevice} starts out. Everything has a working default. */
export interface MockDeviceOptions {
    /** The card's identity. A different one means everything a client cached is void. */
    storeId?: string;
    /** The commit sequence a freshly mounted store reports. */
    commitSequence?: bigint;
    /** Start with no readable flat catalog; only FORMAT with an all-zero expected identity recovers it. */
    formatRecovery?: "unformatted" | "catalog-unreadable";
    deviceInfo?: DeviceInfo;
    /** Total payload bytes the card can hold. A `PUT` past it is `noSpace` with the bytes required. */
    cardBytes?: number;
    /** Entries per `LIST` page. Defaults to {@link LIST_PAGE_ENTRIES}; tests shrink it to page. */
    pageEntries?: number;
    /** Payload bytes per stream record when serving a `GET`. At most {@link MAX_STREAM_PAYLOAD}. */
    streamPayload?: number;
    /**
     * Whether `ARM` may succeed. `false` — the default — is the device's current policy, which
     * refuses with `rejected`; `true` exists so the success shape has a test.
     */
    armPolicy?: "refuse" | "allow";
    /**
     * Verify an upload's CRC as bytes arrive and keep **nothing** — what a device with a 300 MB map
     * coming down the cable actually does, since it sinks to the card and has no RAM to buffer in.
     *
     * Off by default because most tests want to assert on {@link MockDevice.payloadOf}; on for the
     * flat-memory measurement, where a device that buffered the object would be the thing consuming
     * the memory rather than the code under test.
     */
    sinkUploads?: boolean;
}

/**
 * A device that speaks protocol v4 over a {@link DeviceLink}.
 *
 * `run()` starts its control loop and resolves when the link closes; nothing else is needed to have
 * a working device on the other end of a {@link FlatStoreClient}.
 */
export class MockDevice {
    private readonly control: RecordChannel;
    private readonly stream: RecordChannel;

    storeId: string;
    private commitSequence: bigint;
    private readonly cardBytes: number;
    private readonly pageEntries: number;
    private readonly streamPayload: number;
    private readonly armPolicy: "refuse" | "allow";
    private readonly sinkUploads: boolean;
    private formatRecovery: "unformatted" | "catalog-unreadable" | null;

    /** The catalog, kept in `(ObjectId, Revision)` order — the order §3.3 pages in. */
    private readonly catalog: Stored[] = [];
    private nextObjectId = 1n;

    /** The one live transfer, §1's rule made concrete. */
    private live: { requestId: number; opcode: number; abort: AbortController } | null = null;

    /** Non-transport failures from detached work — a real defect, not a disconnect. */
    readonly faults: unknown[] = [];
    /** Every request the device served, in order. Lets a test assert what a flow did *not* send. */
    readonly requestLog: Array<{ opcode: number; requestId: number }> = [];

    private running = false;

    constructor(link: DeviceLink, options: MockDeviceOptions = {}) {
        this.control = new RecordChannel(link.control, MAX_DEVICE_RECORD, MAX_HOST_CONTROL_RECORD);
        this.stream = new RecordChannel(link.stream, MAX_DEVICE_RECORD, MAX_HOST_STREAM_RECORD);
        this.storeId = options.storeId ?? REFERENCE_STORE_ID;
        this.commitSequence = options.commitSequence ?? 1n;
        this.formatRecovery = options.formatRecovery ?? null;
        this.cardBytes = options.cardBytes ?? 8 * 1024 ** 3;
        this.pageEntries = options.pageEntries ?? LIST_PAGE_ENTRIES;
        this.streamPayload = Math.min(options.streamPayload ?? MAX_STREAM_PAYLOAD, MAX_STREAM_PAYLOAD);
        this.armPolicy = options.armPolicy ?? "refuse";
        this.sinkUploads = options.sinkUploads ?? false;
    }

    // --- seeding ---------------------------------------------------------------

    /**
     * Put an object on the card without a `PUT`.
     *
     * `bytes` may be omitted for a catalog row with no payload behind it — the recording ride and the
     * rollback reserve are exactly that on a real device, and a `GET` of either is refused anyway.
     */
    seed(object: {
        objectId?: bigint;
        revision?: bigint;
        kind: ObjectKind;
        displayName?: string;
        flags?: number;
        bytes?: Uint8Array;
        /** Only for a row with no bytes: what the entry should claim. */
        payloadLength?: bigint;
        payloadCrc32?: number;
    }): CatalogEntry {
        const objectId = object.objectId ?? this.nextObjectId;
        const bytes = object.bytes ?? null;
        const meta: CatalogEntry = {
            objectId,
            revision: object.revision ?? 1n,
            payloadLength: bytes ? BigInt(bytes.length) : (object.payloadLength ?? 0n),
            payloadCrc32: bytes ? Crc32.of(bytes) : (object.payloadCrc32 ?? 0),
            kind: object.kind,
            flags: object.flags ?? 0,
            displayName: object.displayName ?? "",
        };
        this.insert({ meta, bytes });
        if (objectId >= this.nextObjectId) this.nextObjectId = objectId + 1n;
        this.commitSequence += 1n;
        return meta;
    }

    /** The whole catalog as the device would list it. */
    get entries(): readonly CatalogEntry[] {
        return this.catalog.map((row) => row.meta);
    }

    /** The bytes the device holds for one object's head revision, or `null`. */
    payloadOf(objectId: bigint): Uint8Array | null {
        return this.head(objectId)?.bytes ?? null;
    }

    /** The card's commit sequence — what a client reads back from `LIST` to see a movement. */
    get sequence(): bigint {
        return this.commitSequence;
    }

    /** Payload bytes the catalog accounts for. The free-space answer, which no opcode serves. */
    get usedBytes(): number {
        return this.catalog.reduce((n, row) => n + Number(row.meta.payloadLength), 0);
    }

    // --- the control loop ------------------------------------------------------

    /** Serve until the link closes. Rejects only on a defect, never on a normal disconnect. */
    async run(): Promise<void> {
        this.running = true;
        while (this.running) {
            let record: Uint8Array;
            try {
                record = await this.control.next();
            } catch {
                this.running = false;
                return;
            }
            try {
                await this.handle(record);
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
        this.live?.abort.abort();
    }

    private async handle(record: Uint8Array): Promise<void> {
        const decoded = decodeRequest(record);
        if (isFailure(decoded)) {
            // §3.1: an unanswerable record gets nothing at all and closes the record stream. A
            // refusable one gets an error response under its own `RequestId`.
            if (decoded.kind === "unanswerable") {
                this.running = false;
                return;
            }
            await this.send(encodeErrorResponse(record[5], decoded.requestId, decoded.refusal));
            return;
        }
        const { requestId, request } = decoded;
        this.requestLog.push({ opcode: request.opcode, requestId });
        switch (request.opcode) {
            case Opcode.List:
                await this.serveList(requestId, request.body);
                return;
            case Opcode.Status:
                await this.serveStatus(requestId, request.body);
                return;
            case Opcode.Remove:
                await this.serveRemove(requestId, request.body);
                return;
            case Opcode.Cancel:
                await this.serveCancel(requestId, request.body.transferRequestId);
                return;
            case Opcode.Arm:
                await this.serveArm(requestId, request.body);
                return;
            case Opcode.Format:
                await this.serveFormat(requestId, request.body);
                return;
            case Opcode.Get:
            case Opcode.Put:
                await this.startTransfer(requestId, request);
                return;
        }
    }

    private send(frame: Uint8Array): Promise<void> {
        return this.control.send(frame);
    }

    private refuse(opcode: number, requestId: number, r: Refusal): Promise<void> {
        return this.send(encodeErrorResponse(opcode, requestId, r));
    }

    // --- LIST (§3.3) ------------------------------------------------------------

    private async serveList(requestId: number, request: Extract<Request, { opcode: 0x01 }>["body"]): Promise<void> {
        if (this.formatRecovery) {
            const detail =
                this.formatRecovery === "unformatted"
                    ? Detail.readOnly.unformatted
                    : Detail.readOnly.catalogUnreadable;
            await this.refuse(Opcode.List, requestId, refusal(ErrorCode.ReadOnly, detail));
            return;
        }
        if (request.cursor && request.cursor.commitSequence !== this.commitSequence) {
            await this.refuse(
                Opcode.List,
                requestId,
                refusal(ErrorCode.CatalogChanged, Detail.catalogChanged.listing, this.commitSequence),
            );
            return;
        }
        const matching = this.catalog
            .map((row) => row.meta)
            .filter((meta) => request.kind === null || meta.kind === request.kind);
        // The cursor is the pair, and the page resumes strictly *after* it.
        const start = request.cursor
            ? matching.findIndex((meta) => after(meta, request.cursor as { objectId: bigint; revision: bigint }))
            : 0;
        const from = start < 0 ? matching.length : start;
        const entries = matching.slice(from, from + this.pageEntries);
        await this.send(
            encodeListResponse(requestId, {
                storeId: this.storeId,
                commitSequence: this.commitSequence,
                entries,
                more: from + entries.length < matching.length,
            }),
        );
    }

    // --- STATUS (§3.4) ----------------------------------------------------------

    private async serveStatus(requestId: number, ref: ObjectRef): Promise<void> {
        const head = this.head(ref.objectId);
        if (!head) {
            await this.send(
                encodeStatusResponse(requestId, {
                    state: ObjectState.Absent,
                    headRevision: 0n,
                    headPayloadLength: 0n,
                    headPayloadCrc32: 0,
                }),
            );
            return;
        }
        await this.send(
            encodeStatusResponse(requestId, {
                state: head.meta.revision === ref.revision ? ObjectState.Committed : ObjectState.Superseded,
                headRevision: head.meta.revision,
                headPayloadLength: head.meta.payloadLength,
                headPayloadCrc32: head.meta.payloadCrc32,
            }),
        );
    }

    // --- REMOVE (§3.7) ----------------------------------------------------------

    private async serveRemove(requestId: number, ref: ObjectRef): Promise<void> {
        const head = this.head(ref.objectId);
        if (!head) {
            await this.refuse(Opcode.Remove, requestId, refusal(ErrorCode.NotFound, Detail.notFound.object));
            return;
        }
        if (head.meta.flags & (EntryFlags.Recording | EntryFlags.Reserved)) {
            // §3.7: stopping a ride and settling an armed update are device-local acts, and freeing
            // either object's extents under the store or the bootloader is what those flags prevent.
            await this.refuse(
                Opcode.Remove,
                requestId,
                refusal(ErrorCode.InvalidRequest, Detail.invalidRequest.badCombination),
            );
            return;
        }
        if (head.meta.revision !== ref.revision) {
            await this.refuse(
                Opcode.Remove,
                requestId,
                refusal(ErrorCode.RevisionConflict, Detail.revisionConflict.headDiffers, head.meta.revision),
            );
            return;
        }
        // One commit removes the entry and a retained previous revision of the same object with it.
        for (let i = this.catalog.length - 1; i >= 0; i--) {
            if (this.catalog[i].meta.objectId === ref.objectId) this.catalog.splice(i, 1);
        }
        this.commitSequence += 1n;
        await this.send(encodeRemoveResponse(requestId, this.commitSequence));
    }

    // --- CANCEL (§3.8) ----------------------------------------------------------

    private async serveCancel(requestId: number, transferRequestId: number): Promise<void> {
        const live = this.live;
        if (!live || live.requestId !== transferRequestId) {
            await this.send(encodeCancelResponse(requestId, false));
            return;
        }
        live.abort.abort();
        // §3.8's cancel **drops** the transfer, so the engine's slot is free at this instant and the
        // answer below already describes a device that can take the next `PUT` or `GET`. Waiting for
        // the runner to unwind first would answer the cancel and then refuse the retry the client
        // sends on the strength of it — `busy` against a transfer that no longer exists.
        this.live = null;
        await this.send(encodeCancelResponse(requestId, true));
        // The cancelled transfer also receives its own `cancelled` error response, sent by its own
        // runner, which the abort above has just woken. That is its tail, not a step the slot waits
        // on.
    }

    // --- ARM (§4) ---------------------------------------------------------------

    private async serveArm(requestId: number, request: { packageObjectId: bigint; expectedRevision: bigint }): Promise<void> {
        const head = this.head(request.packageObjectId);
        if (!head || head.meta.kind !== ObjectKind.UpdatePackage) {
            await this.refuse(Opcode.Arm, requestId, refusal(ErrorCode.NotFound, Detail.notFound.object));
            return;
        }
        if (head.meta.revision !== request.expectedRevision) {
            await this.refuse(
                Opcode.Arm,
                requestId,
                refusal(ErrorCode.RevisionConflict, Detail.revisionConflict.headDiffers, head.meta.revision),
            );
            return;
        }
        if (this.armPolicy === "refuse") {
            // §4 step 1's refusals are all `rejected` with the update kind's detail. The device's
            // current policy refuses every one of them, and saying so on the wire is the honest
            // shape — a device that answered success and never rebooted would be worse.
            await this.refuse(Opcode.Arm, requestId, refusal(ErrorCode.Rejected, 1));
            return;
        }
        const reserve = this.seed({ kind: ObjectKind.RollbackReserve, flags: EntryFlags.Reserved });
        await this.send(
            encodeArmResponse(requestId, { rollbackObjectId: reserve.objectId, commitSequence: this.commitSequence }),
        );
    }

    // --- FORMAT (§3.10) --------------------------------------------------------

    private async serveFormat(
        requestId: number,
        request: { expectedStoreId: string; replacementStoreId: string },
    ): Promise<void> {
        const expected = this.formatRecovery ? "00000000000000000000000000000000" : this.storeId;
        if (request.expectedStoreId !== expected) {
            await this.refuse(
                Opcode.Format,
                requestId,
                refusal(ErrorCode.InvalidRequest, Detail.invalidRequest.badCombination),
            );
            return;
        }
        this.catalog.length = 0;
        this.nextObjectId = 1n;
        this.commitSequence = 1n;
        this.storeId = request.replacementStoreId;
        this.formatRecovery = null;
        await this.send(encodeFormatResponse(requestId, this.storeId));
    }

    // --- transfers (§3.5, §3.6) --------------------------------------------------

    private async startTransfer(requestId: number, request: Request): Promise<void> {
        if (this.live) {
            await this.refuse(
                request.opcode,
                requestId,
                refusal(ErrorCode.Busy, Detail.busy.transfer, BigInt(this.live.requestId)),
            );
            return;
        }
        const abort = new AbortController();
        this.live = { requestId, opcode: request.opcode, abort };
        // Detached: the control loop has to keep serving while bytes move, which is how a `CANCEL`
        // reaches a device that is mid-transfer and how a `LIST` is served beside one.
        this.detach(
            (request.opcode === Opcode.Put
                ? this.receive(requestId, request.body as PutRequest, abort.signal)
                : this.serve(requestId, request.body as ObjectRef, abort.signal)
            ).finally(() => {
                // Only if it is still this transfer's: a cancel releases the slot itself, and the
                // next transfer may already own it by the time this runner unwinds.
                if (this.live?.requestId === requestId) this.live = null;
            }),
        );
    }

    /** §3.5: resolve, stream ascending contiguous records, then answer. */
    private async serve(requestId: number, ref: ObjectRef, signal: AbortSignal): Promise<void> {
        const row = ref.revision === 0n ? this.head(ref.objectId) : this.at(ref.objectId, ref.revision);
        if (!row) {
            await this.refuse(
                Opcode.Get,
                requestId,
                refusal(ErrorCode.NotFound, ref.revision === 0n ? Detail.notFound.object : Detail.notFound.revision),
            );
            return;
        }
        if (row.meta.flags & (EntryFlags.Recording | EntryFlags.Reserved)) {
            // §3.5: the store did not write a reserve's bytes, and a recording ride's length and CRC
            // are zero until the commit that ends it, so serving one would report success over an
            // empty payload.
            await this.refuse(
                Opcode.Get,
                requestId,
                refusal(ErrorCode.InvalidRequest, Detail.invalidRequest.badCombination),
            );
            return;
        }
        const bytes = row.bytes;
        if (!bytes) {
            await this.refuse(Opcode.Get, requestId, refusal(ErrorCode.MediaIo, Detail.mediaIo.read));
            return;
        }
        try {
            for (let at = 0; at < bytes.length; at += this.streamPayload) {
                const payload = bytes.subarray(at, Math.min(at + this.streamPayload, bytes.length));
                // The signal has to reach the write, not only the loop: a device parked on a full
                // endpoint would otherwise keep the host waiting for its answer.
                await this.stream.send(encodeStreamRecord(requestId, BigInt(at), payload), signal);
            }
        } catch {
            await this.refuse(Opcode.Get, requestId, refusal(ErrorCode.Cancelled, Detail.cancelled.byClient));
            return;
        }
        await this.send(
            encodeGetResponse(requestId, {
                revisionServed: row.meta.revision,
                payloadLength: row.meta.payloadLength,
                payloadCrc32: row.meta.payloadCrc32,
            }),
        );
    }

    /** §3.6: admit, consume the stream, verify, commit, answer. */
    private async receive(requestId: number, request: PutRequest, signal: AbortSignal): Promise<void> {
        const admission = this.admit(request);
        if (admission) {
            await this.refuse(Opcode.Put, requestId, admission);
            // §3.6: the device discards frames bearing this `RequestId`. The loopback channel simply
            // queues them, and the client's own reset is what drops them — which is the same
            // observable outcome and needs no drain here.
            return;
        }

        const declared = toSafeNumber(request.payloadLength, "the declared payload length");
        const buffer = this.sinkUploads ? null : new Uint8Array(declared);
        const crc = new Crc32();
        let got = 0;
        try {
            while (got < declared) {
                const record = await this.stream.next(signal);
                const split = splitStreamRecord(record);
                if (!split) {
                    await this.refuse(
                        Opcode.Put,
                        requestId,
                        refusal(ErrorCode.InvalidRequest, Detail.invalidRequest.streamOffset),
                    );
                    return;
                }
                // §3.8: a frame bearing a `RequestId` that is not the live transfer's is discarded
                // in silence.
                if (split.frame.transferRequestId !== requestId) continue;
                if (split.frame.offset !== BigInt(got) || got + split.payload.length > declared) {
                    await this.refuse(
                        Opcode.Put,
                        requestId,
                        refusal(ErrorCode.InvalidRequest, Detail.invalidRequest.streamOffset),
                    );
                    return;
                }
                buffer?.set(split.payload, got);
                crc.update(split.payload);
                got += split.payload.length;
            }
        } catch {
            // A cancelled or dropped upload is discarded whole — transfers restart, never resume —
            // and §3.6's "any break before the commit leaves the card as if nothing happened".
            await this.refuse(Opcode.Put, requestId, refusal(ErrorCode.Cancelled, Detail.cancelled.byClient));
            return;
        }

        if (crc.value() !== request.payloadCrc32) {
            await this.refuse(
                Opcode.Put,
                requestId,
                refusal(ErrorCode.ChecksumFailure, Detail.checksumFailure.payload, BigInt(request.payloadCrc32)),
            );
            return;
        }
        const published = this.commit(request, buffer, declared);
        await this.send(encodePutResponse(requestId, published));
    }

    /**
     * Everything §3.6 decides before a byte is consumed.
     *
     * `null` admits. The order matters and follows the spec's own: what the request *is* (a create
     * or a replace, a kind a client may write), then what the catalog says, then what the card has
     * room for.
     */
    private admit(request: PutRequest): Refusal | null {
        // §3.6: kinds 3 and 8 are produced by the device, and a client that could overwrite a ride
        // mid-recording or a rollback reserve mid-update would be writing where the store and the
        // bootloader already are.
        if (request.kind === ObjectKind.Ride || request.kind === ObjectKind.RollbackReserve) {
            return refusal(ErrorCode.InvalidRequest, Detail.invalidRequest.badCombination);
        }
        let displaced = 0;
        if (request.objectId !== NO_OBJECT) {
            const head = this.head(request.objectId);
            if (!head) {
                return refusal(ErrorCode.RevisionConflict, Detail.revisionConflict.headAbsent);
            }
            if (head.meta.flags & (EntryFlags.Recording | EntryFlags.Reserved)) {
                return refusal(ErrorCode.InvalidRequest, Detail.invalidRequest.badCombination);
            }
            if (head.meta.revision !== request.expectedRevision) {
                return refusal(
                    ErrorCode.RevisionConflict,
                    Detail.revisionConflict.headDiffers,
                    head.meta.revision,
                );
            }
            // A replace frees what it displaces in the same commit, so only the delta has to fit.
            displaced = Number(head.meta.payloadLength);
        }
        const required = toSafeNumber(request.payloadLength, "the declared payload length");
        if (this.usedBytes - displaced + required > this.cardBytes) {
            return refusal(ErrorCode.NoSpace, Detail.noSpace.extents, BigInt(required));
        }
        return null;
    }

    /** One commit publishes the new head and settles what it displaced (§3.6). */
    private commit(request: PutRequest, bytes: Uint8Array | null, length: number): {
        objectId: bigint;
        revision: bigint;
        payloadLength: bigint;
        payloadCrc32: number;
    } {
        const creating = request.objectId === NO_OBJECT;
        const objectId = creating ? this.nextObjectId++ : request.objectId;
        const head = creating ? null : this.head(objectId);
        const revision = head ? head.meta.revision + 1n : 1n;

        // A replace leaves at most what it asked for: any revision the object was already keeping
        // retained is freed, and the displaced one is kept only when the flag asked for it.
        for (let i = this.catalog.length - 1; i >= 0; i--) {
            const row = this.catalog[i];
            if (row.meta.objectId !== objectId) continue;
            if (head && row.meta.revision === head.meta.revision && request.retainPrevious) {
                this.catalog[i] = { ...row, meta: { ...row.meta, flags: row.meta.flags | EntryFlags.Retained } };
                continue;
            }
            this.catalog.splice(i, 1);
        }

        const meta: CatalogEntry = {
            objectId,
            revision,
            payloadLength: BigInt(length),
            payloadCrc32: request.payloadCrc32,
            kind: request.kind,
            flags: 0,
            displayName: request.displayName,
        };
        this.insert({ meta, bytes });
        this.commitSequence += 1n;
        return { objectId, revision, payloadLength: meta.payloadLength, payloadCrc32: meta.payloadCrc32 };
    }

    // --- catalog helpers ---------------------------------------------------------

    /** Insert keeping `(ObjectId, Revision)` order — the order §3.3 pages in. */
    private insert(row: Stored): void {
        const at = this.catalog.findIndex((other) => after(other.meta, row.meta));
        if (at < 0) this.catalog.push(row);
        else this.catalog.splice(at, 0, row);
    }

    /** The greatest revision of an object — its head. */
    private head(objectId: bigint): Stored | null {
        let best: Stored | null = null;
        for (const row of this.catalog) {
            if (row.meta.objectId !== objectId) continue;
            if (!best || row.meta.revision > best.meta.revision) best = row;
        }
        return best;
    }

    private at(objectId: bigint, revision: bigint): Stored | null {
        return this.catalog.find((row) => row.meta.objectId === objectId && row.meta.revision === revision) ?? null;
    }

    /**
     * Let a detached task finish without turning a normal disconnect into an unhandled rejection.
     *
     * A device that is mid-transfer when the host closes the link will fail its next write, and that
     * is the ordinary end of a session, not a defect. Anything that is not a transport failure is
     * kept in {@link faults} so a test can find a real bug rather than have it swallowed.
     */
    private detach(task: Promise<unknown>): void {
        void task.catch((cause: unknown) => {
            if (!(cause instanceof PipeError)) this.faults.push(cause);
        });
    }
}

/** `(ObjectId, Revision)` ordering: is `meta` strictly after `cursor`? */
function after(meta: { objectId: bigint; revision: bigint }, cursor: { objectId: bigint; revision: bigint }): boolean {
    if (meta.objectId !== cursor.objectId) return meta.objectId > cursor.objectId;
    return meta.revision > cursor.revision;
}

/**
 * A client wired to a running {@link MockDevice} — the one-liner every flow test starts from.
 *
 * Seed the device, drive the client, `close()` when done. The device's control loop runs detached;
 * closing the client closes the link, which ends it.
 */
export function loopbackDevice(
    options: LoopbackOptions & MockDeviceOptions & { clientTimeoutMs?: number } = {},
): {
    client: FlatStoreClient;
    device: MockDevice;
    link: LoopbackLink;
    close: () => Promise<void>;
} {
    const link = loopbackLink(options);
    const device = new MockDevice(link.device, options);
    void device.run();
    // `clientTimeoutMs` exists for one kind of test: a device that is enumerated but hung, where
    // the assertion is that a call *ends* rather than what it returns. The client's real default is
    // fifteen seconds, which is right on a wire and useless in a suite.
    const client = new FlatStoreClient(link.host, { timeoutMs: options.clientTimeoutMs });
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
