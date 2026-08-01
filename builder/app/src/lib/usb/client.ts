/**
 * The protocol client: the interface spec's object model driven over a {@link DeviceLink}.
 *
 * One implementation serves both tiers. The hosted site drives it over WebUSB (`webusb.ts`), the
 * desktop app over `nusb` behind Tauri (`lib/desktop/usb.ts`, D4 #909), and CI over an in-memory
 * loopback (`loopback.ts`) — the client cannot tell, because everything below it is two byte pipes.
 * That is the whole point of building this once: a parallel desktop implementation would drift, and
 * the thing it would drift from is a byte-exact wire contract with a device that ships in the field.
 *
 * ## The rules this file exists to hold
 *
 * - **One transfer at a time** (§4.1). A transfer slot is held from the descriptor write until a
 *   *correlated* close has been consumed. A second attempt fails fast rather than interleaving two
 *   objects onto an unframed stream, which would corrupt both.
 * - **An interrupted exchange leaves the pipe unusable** (§4.1). After a cancel, a timeout, a
 *   mismatched or late answer, or a descriptor-open reject, the bulk pipe is reset before the slot
 *   is released — otherwise bytes the device queued before its reject arrived become the first
 *   bytes of the next object.
 * - **The CRC is whole-object and verified once** (§6). Uploads announce it up front, downloads
 *   verify it as bytes arrive; a mismatch rejects the object, it never commits a repair.
 * - **Reads are not messages.** Every bulk read loop accumulates until the announced `total_len`,
 *   because a bulk endpoint delivers whatever segmentation it likes.
 */

import { Crc32 } from "./crc32";
import {
    decodeRideList,
    decodeRouteList,
    decodeTripList,
    type RideListEntry,
    type RouteListEntry,
    type TripListEntry,
} from "./objects";
import { PipeError, throwIfAborted, type BytePipe, type DeviceLink } from "./pipe";
import {
    COMMAND_STATUS_NAMES,
    Command,
    CommandStatus,
    NEW_OBJECT_ID,
    ObjectType,
    Op,
    PROTOCOL_VERSION,
    SINGLETON_OBJECT_ID,
    TRANSFER_STATUS_NAMES,
    TransferStatus,
    decodeConfig,
    decodeStatusMessage,
    decodeVersionRead,
    encodeAckRides,
    encodeConfig,
    encodeDeleteObject,
    encodeForgetBond,
    encodeInstallFw,
    encodeSetClock,
    encodeSetRouteRetention,
    encodeTransferControl,
    type DeviceConfig,
    type StatusMessage,
    type TransferControl,
    type VersionRead,
} from "./protocol";
import {
    DeviceFrame,
    HostFrame,
    decodeCardFree,
    decodeDeviceInfo,
    decodeFrame,
    encodeFrame,
    type DeviceInfo,
} from "./transport";

/**
 * Why a device operation failed.
 *
 * The six transfer statuses and the five command statuses collapse onto these, because what a
 * caller does about them differs: `storage-full` asks the rider to delete something, `crc-mismatch`
 * retries, `link` re-plugs, `unsupported-command` means the device predates a feature and the UI
 * degrades rather than complains.
 */
export type DeviceErrorCode =
    | "protocol-version"
    | "link"
    | "timeout"
    | "aborted"
    | "busy"
    | "crc-mismatch"
    | "storage-full"
    | "not-found"
    | "unsupported-command"
    | "device-error"
    | "protocol";

/** A failure at the object layer. `status` carries the raw wire code where there was one. */
export class DeviceError extends Error {
    readonly code: DeviceErrorCode;
    readonly status?: number;

    constructor(code: DeviceErrorCode, message: string, options?: { cause?: unknown; status?: number }) {
        super(message, options);
        this.name = "DeviceError";
        this.code = code;
        this.status = options?.status;
    }
}

/** How many bytes an upload hands the bulk pipe at a time. Sized for a high-speed bulk endpoint;
 *  the pipe is free to re-segment, and the device must accept any segmentation anyway. */
export const DEFAULT_CHUNK_SIZE = 16 * 1024;

/**
 * How long to wait for a device answer before giving up.
 *
 * 15 s, matching the bounded status-wait the iOS app settled on after an unbounded one wedged the
 * transfer slot forever (trips epic #526). A device that has stopped answering must produce an
 * error, never a spinner that outlives the ride.
 */
export const DEFAULT_TIMEOUT_MS = 15_000;

/**
 * The two checks the upload loop makes between chunks, handed to a source that does its own I/O.
 *
 * A {@link ObjectSource.sendTo} implementation is *replacing* that loop, so it inherits the loop's
 * obligations rather than being trusted to have none.
 */
export interface SendHooks {
    /** The caller's cancel. Also honoured by {@link check}, so a source that only polls is correct. */
    signal?: AbortSignal;
    /** Bytes handed to the transport so far, for the progress bar. */
    onProgress?: (done: number) => void;
    /**
     * Throws if the send must stop **now**.
     *
     * Two reasons, and the second is the one that is easy to miss: the caller cancelled, or the
     * device has already rejected the descriptor. A descriptor-open reject (storage full, a size
     * ceiling, busy) is asynchronous — it arrives on the control plane while these bytes are
     * queued — and a source that never checks would push a whole 300 MB map at a device that said
     * no on the first megabyte. Call it at least as often as progress is reported.
     */
    check(): void;
}

/** An object to upload, with its length and whole-object CRC known before the first byte moves —
 *  the descriptor announces both (§4.2). */
export interface ObjectSource {
    readonly totalLen: number;
    readonly crc32: number;
    /** Yield the object's bytes in order, in slices of at most `chunkSize`. */
    chunks(chunkSize: number): AsyncIterable<Uint8Array>;
    /**
     * Optional: move the whole object into `pipe` without its bytes passing through this process.
     *
     * A source has this only where the bytes already live somewhere the *transport* can reach on
     * its own — which today means one thing: the desktop app, where the file is on the same disk as
     * the Rust process that owns the USB endpoint (D4 #909). Routing a several-hundred-megabyte map
     * through the webview to hand it straight back would copy every byte twice for nothing, and
     * #894 says so explicitly.
     *
     * Resolves with the number of bytes the transport took. The caller still checks that against
     * the announced length, so a short send is caught here rather than becoming a half-written
     * object the device is asked to commit.
     */
    sendTo?(pipe: BytePipe, hooks: SendHooks): Promise<number>;
}

/** An in-memory object: one CRC pass now, then straight slices. */
export function bytesSource(bytes: Uint8Array): ObjectSource {
    return {
        totalLen: bytes.length,
        crc32: Crc32.of(bytes),
        async *chunks(chunkSize: number) {
            for (let at = 0; at < bytes.length; at += chunkSize) {
                yield bytes.subarray(at, Math.min(at + chunkSize, bytes.length));
            }
        },
    };
}

/**
 * A `Blob` — a fetched map, a picked file — without ever holding it twice.
 *
 * The descriptor needs the CRC before the first byte streams and the object cannot be re-derived
 * from a suffix, so the blob is read **twice**: once to fingerprint, once to send. That is the
 * right trade for a 200 MB regional map, where the alternative is a second 200 MB copy in the tab's
 * heap. Blobs are backed by the browser's own storage, so the second pass is cheap.
 */
export async function blobSource(blob: Blob): Promise<ObjectSource> {
    const crc = new Crc32();
    for await (const chunk of streamChunks(blob.stream())) crc.update(chunk);
    return {
        totalLen: blob.size,
        crc32: crc.value(),
        async *chunks(chunkSize: number) {
            for await (const chunk of streamChunks(blob.stream())) {
                for (let at = 0; at < chunk.length; at += chunkSize) {
                    yield chunk.subarray(at, Math.min(at + chunkSize, chunk.length));
                }
            }
        },
    };
}

async function* streamChunks(stream: ReadableStream<Uint8Array>): AsyncGenerator<Uint8Array> {
    const reader = stream.getReader();
    try {
        for (;;) {
            const { done, value } = await reader.read();
            if (done) return;
            if (value.length) yield value;
        }
    } finally {
        reader.releaseLock();
    }
}

/** Per-call knobs shared by both transfer directions. */
export interface TransferOptions {
    /** Cancel the transfer. A cancelled transfer resets the bulk pipe before releasing the slot. */
    signal?: AbortSignal;
    /** Called as bytes move, for a progress bar. `done` and `total` are byte counts. */
    onProgress?: (done: number, total: number) => void;
    /** Override {@link DEFAULT_CHUNK_SIZE} for one upload. */
    chunkSize?: number;
}

/** What an upload committed to. */
export interface UploadResult {
    /** The device's id for the object — the **assigned** id when the upload was a fresh one. */
    objectId: number;
    /** Bytes the device reports durable. Always equals the announced length on a commit. */
    committedOffset: number;
}

/** Options for a {@link ProtocolClient}. */
export interface ClientOptions {
    /** Bound on every wait for a device answer. Defaults to {@link DEFAULT_TIMEOUT_MS}. */
    timeoutMs?: number;
}

/**
 * `ackRides` ids per `command` write.
 *
 * The reference firmware accepts ≤ 31 ids in a 64-byte GATT value; the USB frame spends one byte on
 * its selector, so 30 keeps the whole frame inside a 64-byte packet. The command is idempotent and
 * order-free, so splitting a long list across writes is free.
 */
const ACK_RIDES_PER_WRITE = 30;

/** How long an abort waits for the device's `aborted` result before giving up and resetting anyway. */
const ABORT_ACK_TIMEOUT_MS = 2_000;

/**
 * A connected device, speaking the interface spec over two byte pipes.
 *
 * Construct it around an open {@link DeviceLink}; it starts a read loop on the control pipe
 * immediately, so unsolicited edges — `storeChanged`, a `transferResult` that overtakes the last
 * bulk bytes — are never missed because nobody happened to be reading.
 */
export class ProtocolClient {
    private readonly link: DeviceLink;
    private readonly timeoutMs: number;

    private readonly statuses = new Mailbox<StatusMessage>();
    private readonly identities = new Mailbox<Uint8Array>();
    private readonly deviceInfos = new Mailbox<Uint8Array>();
    private readonly configs = new Mailbox<Uint8Array>();
    private readonly cardFreeReplies = new Mailbox<Uint8Array>();

    private readonly storeListeners = new Set<(type: number, revision: number) => void>();

    /** Held for the whole of one transfer, descriptor write through correlated close (§4.1). */
    private transferBusy = false;
    /** Set once the active exchange has consumed a terminal `transferResult`, so a failure after
     *  that point does not send the device an abort for a transfer it has already closed. */
    private transferClosed = false;
    /** Serialises `command` writes so a `commandResult` correlates by its echoed command byte. */
    private commandChain: Promise<unknown> = Promise.resolve();

    private closed = false;
    private readonly readLoop: Promise<void>;

    constructor(link: DeviceLink, options: ClientOptions = {}) {
        this.link = link;
        this.timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
        this.readLoop = this.pumpControl();
    }

    /** Subscribe to `storeChanged` edges — the sole change signal (§4.5). Returns an unsubscriber. */
    onStoreChanged(listener: (type: number, revision: number) => void): () => void {
        this.storeListeners.add(listener);
        return () => this.storeListeners.delete(listener);
    }

    /**
     * The identity read (§1) — the first thing every connection does.
     *
     * A version other than this client's is surfaced and stopped on, never best-effort decoded: the
     * spec has no dual-version serving, so a mismatch means the two ends disagree about every
     * layout below this line.
     */
    async identity(signal?: AbortSignal): Promise<VersionRead> {
        await this.sendControl(encodeFrame(HostFrame.IdentityRead), signal);
        const read = decodeVersionRead(await this.identities.take(this.timeoutMs, signal, "the identity read"));
        if (read.version !== PROTOCOL_VERSION) {
            throw new DeviceError(
                "protocol-version",
                `This device speaks protocol v${read.version}; this page speaks v${PROTOCOL_VERSION}. ` +
                    "Update the device firmware, or reload the page for a newer build.",
            );
        }
        return read;
    }

    /** The Device Information strings (§3.1) — the running firmware version, board id and serial. */
    async deviceInfo(signal?: AbortSignal): Promise<DeviceInfo> {
        await this.sendControl(encodeFrame(HostFrame.DeviceInfoRead), signal);
        return decodeDeviceInfo(await this.deviceInfos.take(this.timeoutMs, signal, "the device-info read"));
    }

    /** Read the Config object (§7.3). */
    async readConfig(signal?: AbortSignal): Promise<DeviceConfig> {
        await this.sendControl(encodeFrame(HostFrame.ConfigRead), signal);
        return decodeConfig(await this.configs.take(this.timeoutMs, signal, "the config read"));
    }

    /** Write the Config object whole (§7.3). Renaming the device *is* a Config write. */
    async writeConfig(config: DeviceConfig, signal?: AbortSignal): Promise<void> {
        await this.sendControl(encodeFrame(HostFrame.ConfigWrite, encodeConfig(config)), signal);
    }

    /** Free bytes on the currently mounted SD card, or `null` when no readable card is present. */
    async cardFreeBytes(signal?: AbortSignal): Promise<number | null> {
        await this.sendControl(encodeFrame(HostFrame.CardFreeRead), signal);
        return decodeCardFree(await this.cardFreeReplies.take(this.timeoutMs, signal, "the card-space read"));
    }

    // --- transfers ------------------------------------------------------------

    /**
     * Upload an object: descriptor announce, then the raw bytes, then the device's verdict.
     *
     * `objectId` is {@link NEW_OBJECT_ID} for a fresh object — the device assigns one and reports it
     * in the result — or an existing id to replace that object atomically. A fresh upload whose
     * length and CRC match an object the device already holds is answered `committed` with the
     * *existing* id and nothing is stored, which is what makes a retry after a lost ack convergent
     * rather than a source of silent twins.
     */
    async upload(
        type: ObjectType,
        objectId: number,
        source: ObjectSource | Uint8Array,
        options: TransferOptions = {},
    ): Promise<UploadResult> {
        const src = source instanceof Uint8Array ? bytesSource(source) : source;
        const { signal, onProgress, chunkSize = DEFAULT_CHUNK_SIZE } = options;
        return this.withTransferSlot(() => ({ type, objectId }), async () => {
            await this.sendDescriptor(
                { op: Op.Upload, type, objectId, totalLen: src.totalLen, crc32: src.crc32 },
                signal,
            );

            let sent = 0;
            onProgress?.(0, src.totalLen);
            const check = () => this.checkUploadOpen(signal);
            if (src.sendTo) {
                // The source owns the transport for the length of the object (the desktop app's
                // file-path plane). It gets the same two checks the loop below makes, and its
                // answer is still measured against the announced length underneath.
                sent = await src.sendTo(this.link.bulk, {
                    signal,
                    onProgress: (done) => onProgress?.(done, src.totalLen),
                    check,
                });
            } else {
                for await (const chunk of src.chunks(chunkSize)) {
                    check();
                    await this.link.bulk.write(chunk, signal);
                    sent += chunk.length;
                    onProgress?.(sent, src.totalLen);
                }
            }
            if (sent !== src.totalLen) {
                throw new DeviceError(
                    "protocol",
                    `the upload source yielded ${sent} bytes but announced ${src.totalLen}.`,
                );
            }

            const result = await this.awaitTransferResult(signal, "upload");
            this.transferClosed = true;
            if (result.status !== TransferStatus.Committed) throw this.transferFailure(result, "upload");
            // A fresh upload's result carries the assigned id, so correlation is only meaningful for
            // a named one — but a *named* upload answered about some other object is a stale or
            // crossed answer, and the channel it came from cannot be trusted for the next object.
            if (objectId !== NEW_OBJECT_ID && result.objectId !== objectId) {
                throw new DeviceError(
                    "protocol",
                    `the device closed object ${result.objectId} while ${objectId} was uploading.`,
                );
            }
            if (result.committedOffset !== src.totalLen) {
                throw new DeviceError(
                    "protocol",
                    `the device committed ${result.committedOffset} of ${src.totalLen} announced bytes.`,
                );
            }
            return { objectId: result.objectId, committedOffset: result.committedOffset };
        }, options.signal);
    }

    /**
     * Abandon a volume set between whole-file transfers.
     *
     * Cancelling an active shard already sends `op=abort`; this is the other
     * edge: the worker or host can fail after one shard committed and before
     * the next descriptor opens. Naming any valid part of the in-flight shape
     * asks the device to delete every staged shard.
     */
    async abandonMapSet(shardObjectId: number): Promise<void> {
        await this.withTransferSlot(
            () => ({ type: ObjectType.MapShard, objectId: shardObjectId }),
            async () => {
                await this.sendDescriptor({
                    op: Op.Abort,
                    type: ObjectType.MapShard,
                    objectId: shardObjectId,
                    totalLen: 0,
                    crc32: 0,
                });
                await this.awaitTransferResult(undefined, "set-abandon");
                this.transferClosed = true;
            },
        );
    }

    /**
     * Download an object: request, announce, bytes, verify, close.
     *
     * The CRC is folded in as slices arrive — a mismatch is the peer's to detect and reject, and it
     * is detected before anything is handed back to a caller, so a corrupt ride can never reach a
     * GPX export.
     */
    async download(type: ObjectType, objectId: number, options: TransferOptions = {}): Promise<Uint8Array> {
        const { signal, onProgress } = options;
        return this.withTransferSlot(() => ({ type, objectId }), async () => {
            await this.sendDescriptor({ op: Op.Download, type, objectId, totalLen: 0, crc32: 0 }, signal);

            const announce = await this.awaitAnnounce(type, objectId, signal);
            const out = new Uint8Array(announce.totalLen);
            const crc = new Crc32();
            let got = 0;
            onProgress?.(0, announce.totalLen);
            while (got < announce.totalLen) {
                const slice = await this.link.bulk.read(signal);
                // A bulk endpoint hands over whatever segmentation it likes, including more than
                // the object has left. Surplus bytes mean the two ends disagree about the object's
                // length — the stream is no longer interpretable, so stop rather than truncate.
                if (got + slice.length > announce.totalLen) {
                    throw new DeviceError(
                        "protocol",
                        `the device sent ${got + slice.length} bytes for a ${announce.totalLen}-byte object.`,
                    );
                }
                out.set(slice, got);
                crc.update(slice);
                got += slice.length;
                onProgress?.(got, announce.totalLen);
            }
            if (crc.value() !== announce.crc32) {
                throw new DeviceError(
                    "crc-mismatch",
                    `the downloaded object failed its checksum (announced ${hex(announce.crc32)}, ` +
                        `computed ${hex(crc.value())}). Nothing was kept; try again.`,
                );
            }

            const result = await this.awaitTransferResult(signal, "download");
            this.transferClosed = true;
            if (result.status !== TransferStatus.Committed) throw this.transferFailure(result, "download");
            return out;
        }, options.signal);
    }

    /** The route catalog (§7.4), with the device's truncation flag intact. */
    async listRoutes(options?: TransferOptions): Promise<{ entries: RouteListEntry[]; truncated: boolean }> {
        return this.list(ObjectType.RouteList, decodeRouteList, options);
    }

    /** The ride catalog (§7.4). */
    async listRides(options?: TransferOptions): Promise<{ entries: RideListEntry[]; truncated: boolean }> {
        return this.list(ObjectType.RideList, decodeRideList, options);
    }

    /** The trip catalog (§7.4). */
    async listTrips(options?: TransferOptions): Promise<{ entries: TripListEntry[]; truncated: boolean }> {
        return this.list(ObjectType.TripList, decodeTripList, options);
    }

    private async list<T>(
        type: ObjectType,
        decode: (data: Uint8Array) => { header: { count: number; total: number }; entries: T[] },
        options?: TransferOptions,
    ): Promise<{ entries: T[]; truncated: boolean }> {
        const bytes = await this.download(type, SINGLETON_OBJECT_ID, options);
        const { header, entries } = decode(bytes);
        return { entries, truncated: header.total > header.count };
    }

    // --- commands (§4.4) ------------------------------------------------------

    /** Delete a stored route or trip. A trip delete never cascades — its routes become top-level. */
    async deleteObject(type: ObjectType, objectId: number, signal?: AbortSignal): Promise<void> {
        await this.command(encodeDeleteObject(type, objectId), signal);
    }

    /**
     * Flag rides as durably held off the device.
     *
     * **Not for the browser.** #894 defines `synced` as "a durable copy exists off the device", and
     * a one-shot GPX download the user may cancel is not that — the hosted tier never acks, which is
     * why C5 (#904) exports without calling this. The desktop app does ack, *after fsync* (E1 #911),
     * because acking on transfer completion starts an expiry countdown against a ride that is not
     * yet on disk — the single way this feature can lose data.
     *
     * Monotonic and idempotent, so a long list is split across writes and re-sent every connect.
     */
    async ackRides(rideIds: readonly number[], signal?: AbortSignal): Promise<number> {
        let flagged = 0;
        for (let at = 0; at < rideIds.length; at += ACK_RIDES_PER_WRITE) {
            const batch = rideIds.slice(at, at + ACK_RIDES_PER_WRITE);
            const result = await this.command(encodeAckRides(batch), signal);
            flagged += result.detail;
        }
        return flagged;
    }

    /**
     * Ask the device to install its staged `UPDATE.BIN`.
     *
     * Returning `ok` means the *request* was accepted. The device then shows a confirm card and
     * installs only on a physical Select press — a page can stage an image, never install one.
     */
    async installFw(signal?: AbortSignal): Promise<void> {
        await this.command(encodeInstallFw(), signal);
    }

    /** Ask the device to dissolve its side of a BLE bond (§4.4 cmd 4). */
    async forgetBond(signal?: AbortSignal): Promise<void> {
        await this.command(encodeForgetBond(), signal);
    }

    /**
     * Stamp the device's trusted wall clock (§4.4 cmd 5).
     *
     * The device has no RTC and will not expire or stamp anything from a clock it merely resumed
     * from flash. Every connected peer sends this, so `date` defaults to now and the offset to this
     * machine's — with DST already folded in, because the peer is the timezone oracle.
     */
    async setClock(date: Date = new Date(), offsetMinutes?: number, signal?: AbortSignal): Promise<void> {
        const utc = Math.floor(date.getTime() / 1000);
        // `getTimezoneOffset` counts minutes *behind* UTC; the wire counts minutes ahead.
        const offset = offsetMinutes ?? -date.getTimezoneOffset();
        await this.command(encodeSetClock(utc, offset), signal);
    }

    /** Set a stored route's retention level without re-uploading it (§4.4 cmd 6). */
    async setRouteRetention(objectId: number, retention: number, signal?: AbortSignal): Promise<void> {
        await this.command(encodeSetRouteRetention(objectId, retention), signal);
    }

    /**
     * Write one `command` and wait for its `commandResult`.
     *
     * Serialised, so the result's echoed command byte is an unambiguous correlation. An
     * `unknown command` answer is not a failure of the link — it means this device predates the
     * feature, and the spec's compat posture is for the caller to degrade gracefully.
     */
    command(bytes: Uint8Array, signal?: AbortSignal): Promise<{ status: CommandStatus; detail: number }> {
        const run = async () => {
            await this.sendControl(encodeFrame(HostFrame.Command, bytes), signal);
            const result = await this.awaitCommandResult(bytes[0], signal);
            if (result.status !== CommandStatus.Ok) {
                throw new DeviceError(
                    result.status === CommandStatus.UnknownCommand
                        ? "unsupported-command"
                        : result.status === CommandStatus.NotFound
                          ? "not-found"
                          : result.status === CommandStatus.Busy
                            ? "busy"
                            : "device-error",
                    `command ${commandName(bytes[0])} was answered "${COMMAND_STATUS_NAMES[result.status]}".`,
                    { status: result.status },
                );
            }
            return { status: result.status, detail: result.detail };
        };
        // Chain onto the previous command whatever its outcome, so one failure doesn't wedge the
        // queue — but keep the returned promise the caller's own.
        const chained = this.commandChain.then(run, run);
        this.commandChain = chained.catch(() => undefined);
        return chained;
    }

    /** Close both pipes and fail every waiter. Idempotent. */
    async close(): Promise<void> {
        this.closed = true;
        await this.link.close();
        await this.readLoop;
        this.failWaiters(new DeviceError("link", "The device link was closed."));
    }

    // --- the control read loop ------------------------------------------------

    /**
     * The single reader on the control pipe.
     *
     * Everything the device says arrives here and is filed by kind, so a `transferResult` that
     * overtakes the last bulk bytes — the two pipes are independent — is waiting in its mailbox
     * rather than lost. When the pipe dies (an unplug), every waiter is failed at once instead of
     * being left to time out: that difference is a one-second error message versus a fifteen-second
     * spinner.
     */
    private async pumpControl(): Promise<void> {
        for (;;) {
            let frame: Uint8Array;
            try {
                frame = await this.link.control.read();
            } catch (cause) {
                const closed = cause instanceof PipeError && cause.code === "closed";
                this.failWaiters(
                    closed && this.closed
                        ? new DeviceError("link", "The device link was closed.")
                        : new DeviceError("link", "The device disconnected.", { cause }),
                );
                return;
            }
            try {
                this.dispatch(decodeFrame(frame));
            } catch {
                // A frame this build cannot parse is ignored, never fatal: the spec's forward
                // compatibility rule is that unknown messages are skipped, and a link that dropped
                // because a newer firmware added a field would be a far worse failure than a
                // notification nobody read.
            }
        }
    }

    private dispatch(frame: { selector: number; payload: Uint8Array }): void {
        switch (frame.selector) {
            case DeviceFrame.Status: {
                const msg = decodeStatusMessage(frame.payload);
                if (!msg) return; // unknown discriminator — ignored by contract
                if (msg.msg === "storeChanged") {
                    for (const listener of this.storeListeners) listener(msg.type, msg.revision);
                    return;
                }
                this.statuses.push(msg);
                return;
            }
            case DeviceFrame.Identity:
                this.identities.push(frame.payload);
                return;
            case DeviceFrame.DeviceInfo:
                this.deviceInfos.push(frame.payload);
                return;
            case DeviceFrame.Config:
                this.configs.push(frame.payload);
                return;
            case DeviceFrame.CardFree:
                this.cardFreeReplies.push(frame.payload);
                return;
            default:
                return; // unknown selector — same forward-compat posture
        }
    }

    private failWaiters(error: DeviceError): void {
        for (const box of [this.statuses, this.identities, this.deviceInfos, this.configs, this.cardFreeReplies]) {
            box.fail(error);
        }
    }

    // --- transfer plumbing ----------------------------------------------------

    /**
     * Run `body` holding the single transfer slot, resetting the bulk pipe on any unhappy exit.
     *
     * That reset is not defensive tidying: §4.1 says an exchange which does not reach its
     * correlated close leaves the channel unusable, and the bytes still in it belong to an object
     * nobody is reading. Skipping it works on a loopback and desynchronises real hardware.
     */
    private async withTransferSlot<T>(
        descriptorOf: () => { type: ObjectType; objectId: number },
        body: () => Promise<T>,
        signal?: AbortSignal,
    ): Promise<T> {
        if (this.transferBusy) {
            throw new DeviceError("busy", "Another transfer is already running. Wait for it to finish.");
        }
        if (signal?.aborted) throw new DeviceError("aborted", "The transfer was cancelled.", { cause: signal.reason });
        this.transferBusy = true;
        this.transferClosed = false;
        // Anything the device said about a previous, already-closed exchange is stale by
        // definition; clearing it here keeps a late answer from correlating with this transfer.
        this.statuses.drain();
        try {
            return await body();
        } catch (cause) {
            // The device only learns about a peer-side cancel or timeout if we tell it: §4.2's
            // `op = 3` is what makes it drain and discard the partial. A device-originated reject
            // has already cleared its own gate, so an abort then would name a transfer that no
            // longer exists.
            if (!this.transferClosed) await this.sendAbort(descriptorOf());
            await this.resetBulk();
            throw asDeviceError(cause);
        } finally {
            this.transferBusy = false;
        }
    }

    /**
     * Best-effort `op = 3`, then wait for the device to say it has stopped.
     *
     * The wait is the load-bearing half. A device that is mid-stream keeps pushing bytes for as
     * long as it takes the abort to reach it, so resetting the pipe the instant we send one would
     * simply clear it in front of the tail. `transferResult(aborted)` is the device confirming it
     * has drained and discarded the partial, which is the moment the pipe is safe to reset. Bounded
     * short, because this runs on the failure path and the caller is already waiting.
     */
    private async sendAbort(target: { type: ObjectType; objectId: number }): Promise<void> {
        try {
            await this.sendDescriptor({ op: Op.Abort, ...target, totalLen: 0, crc32: 0 });
            await this.statuses.take(Math.min(this.timeoutMs, ABORT_ACK_TIMEOUT_MS), undefined, "the abort result");
        } catch {
            // A device that is gone, or one that never answers, is no reason to hide the caller's
            // original error — the reset that follows is the backstop either way.
        }
    }

    /**
     * May the upload keep pushing bytes?
     *
     * The two conditions the chunk loop used to check inline, hoisted so a source that streams
     * itself ({@link ObjectSource.sendTo}) makes exactly the same checks rather than a similar set.
     */
    private checkUploadOpen(signal?: AbortSignal): void {
        throwIfAborted(signal, "the upload");
        // A descriptor-open reject (storage full, a size ceiling, busy) is asynchronous: the device
        // answers while these bytes are already queued. Stop at the first sign of it rather than
        // pushing a whole map at a device that has said no.
        const early = this.statuses.tryTake(isTransferResult);
        if (early) {
            this.transferClosed = true;
            throw this.transferFailure(early, "upload");
        }
    }

    private async resetBulk(): Promise<void> {
        try {
            await this.link.bulk.reset();
        } catch {
            // The pipe is already gone — the caller's original error is the interesting one.
        }
    }

    private async sendDescriptor(descriptor: TransferControl, signal?: AbortSignal): Promise<void> {
        await this.sendControl(encodeFrame(HostFrame.TransferControl, encodeTransferControl(descriptor)), signal);
    }

    private async sendControl(frame: Uint8Array, signal?: AbortSignal): Promise<void> {
        if (this.closed) throw new DeviceError("link", "The device link is closed.");
        try {
            await this.link.control.write(frame, signal);
        } catch (cause) {
            throw asDeviceError(cause);
        }
    }

    private async awaitAnnounce(
        type: ObjectType,
        objectId: number,
        signal?: AbortSignal,
    ): Promise<TransferControl> {
        const msg = await this.statuses.take(this.timeoutMs, signal, "the download announce");
        if (msg.msg === "transferResult") {
            // A reject instead of an announce: the device never armed the transfer, so its gate is
            // already clear and an abort would name nothing.
            this.transferClosed = true;
            throw this.transferFailure(msg, "download");
        }
        if (msg.msg !== "downloadAnnounce") {
            throw new DeviceError("protocol", `expected a download announce, got a ${msg.msg}.`);
        }
        const d = msg.descriptor;
        if (d.type !== type || d.objectId !== objectId) {
            throw new DeviceError(
                "protocol",
                `the device announced object ${d.objectId} of type ${d.type}, not ${objectId} of type ${type}.`,
            );
        }
        return d;
    }

    private async awaitTransferResult(signal: AbortSignal | undefined, what: string) {
        const msg = await this.statuses.take(this.timeoutMs, signal, `the ${what} result`);
        if (msg.msg !== "transferResult") {
            throw new DeviceError("protocol", `expected a transfer result, got a ${msg.msg}.`);
        }
        return msg;
    }

    private async awaitCommandResult(commandByte: number, signal?: AbortSignal) {
        const msg = await this.statuses.take(this.timeoutMs, signal, "the command result");
        if (msg.msg !== "commandResult") {
            throw new DeviceError("protocol", `expected a command result, got a ${msg.msg}.`);
        }
        if (msg.command !== commandByte) {
            throw new DeviceError(
                "protocol",
                `the device answered command ${msg.command} while ${commandByte} was outstanding.`,
            );
        }
        return msg;
    }

    /** Map a non-committed `transferResult` onto a caller-facing error. */
    private transferFailure(msg: Extract<StatusMessage, { msg: "transferResult" }>, what: string): DeviceError {
        const name = TRANSFER_STATUS_NAMES[msg.status];
        switch (msg.status) {
            case TransferStatus.CrcMismatch:
                return new DeviceError("crc-mismatch", `The device rejected the ${what}: checksum mismatch. ` +
                    "Nothing was stored — try again.", { status: msg.status });
            case TransferStatus.StorageFull:
                return new DeviceError("storage-full", "The device's catalog is full. Delete something on the " +
                    "device and try again.", { status: msg.status });
            case TransferStatus.NotFound:
                return new DeviceError("not-found", `The device does not have that object.`, { status: msg.status });
            case TransferStatus.Busy:
                return new DeviceError("busy", "The device is already busy with another transfer.", {
                    status: msg.status,
                });
            case TransferStatus.Aborted:
                return new DeviceError("aborted", `The ${what} was aborted.`, { status: msg.status });
            default:
                return new DeviceError("device-error", `The device answered "${name}" to the ${what}.`, {
                    status: msg.status,
                });
        }
    }
}

/** Normalise a pipe-level or unknown failure into a {@link DeviceError}. */
function asDeviceError(cause: unknown): DeviceError {
    if (cause instanceof DeviceError) return cause;
    if (cause instanceof PipeError) {
        if (cause.code === "aborted") return new DeviceError("aborted", "The transfer was cancelled.", { cause });
        if (cause.code === "closed") return new DeviceError("link", "The device disconnected.", { cause });
        return new DeviceError("device-error", cause.message, { cause });
    }
    return new DeviceError("device-error", cause instanceof Error ? cause.message : String(cause), { cause });
}

function isTransferResult(m: StatusMessage): m is Extract<StatusMessage, { msg: "transferResult" }> {
    return m.msg === "transferResult";
}

function commandName(byte: number): string {
    const found = Object.entries(Command).find(([, v]) => v === byte);
    return found ? found[0] : String(byte);
}

function hex(v: number): string {
    return `0x${v.toString(16).toUpperCase().padStart(8, "0")}`;
}

/**
 * A FIFO of device answers with waiters attached.
 *
 * The device may answer before anyone asks — a `transferResult` can overtake the last bulk bytes,
 * and both pipes run independently — so the queue holds messages, and a `take` that arrives late
 * finds its answer already there. Failure is sticky: once the link dies every later `take` fails
 * immediately rather than waiting out a timeout on a device that is unplugged.
 */
class Mailbox<T> {
    private readonly queue: T[] = [];
    private readonly waiters: Array<{ resolve: (v: T) => void; reject: (e: unknown) => void }> = [];
    private failure: unknown = null;

    push(value: T): void {
        const waiter = this.waiters.shift();
        if (waiter) waiter.resolve(value);
        else this.queue.push(value);
    }

    /** Take a queued message matching `predicate`, or `null` if none is waiting. Never blocks. */
    tryTake<U extends T>(predicate: (v: T) => v is U): U | null {
        const i = this.queue.findIndex(predicate);
        if (i < 0) return null;
        return this.queue.splice(i, 1)[0] as U;
    }

    /** Drop everything queued — answers to an exchange that is already over. */
    drain(): void {
        this.queue.length = 0;
    }

    /** Fail every current and future waiter. */
    fail(error: unknown): void {
        this.failure = error;
        while (this.waiters.length) this.waiters.shift()?.reject(error);
    }

    take(timeoutMs: number, signal: AbortSignal | undefined, what: string): Promise<T> {
        if (this.queue.length) return Promise.resolve(this.queue.shift() as T);
        if (this.failure) return Promise.reject(this.failure);
        throwIfAborted(signal, what);
        return new Promise<T>((resolve, reject) => {
            const entry = {
                resolve: (v: T) => {
                    cleanup();
                    resolve(v);
                },
                reject: (e: unknown) => {
                    cleanup();
                    reject(e);
                },
            };
            const timer = setTimeout(() => {
                remove();
                cleanup();
                reject(new DeviceError("timeout", `The device did not answer ${what} within ${timeoutMs} ms.`));
            }, timeoutMs);
            const onAbort = () => {
                remove();
                cleanup();
                reject(new DeviceError("aborted", `Waiting for ${what} was cancelled.`, { cause: signal?.reason }));
            };
            const remove = () => {
                const i = this.waiters.indexOf(entry);
                if (i >= 0) this.waiters.splice(i, 1);
            };
            const cleanup = () => {
                clearTimeout(timer);
                signal?.removeEventListener("abort", onAbort);
            };
            signal?.addEventListener("abort", onAbort, { once: true });
            this.waiters.push(entry);
        });
    }
}
