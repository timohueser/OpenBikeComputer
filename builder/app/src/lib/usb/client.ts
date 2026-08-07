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

/**
 * How many bytes an upload hands the bulk pipe at a time.
 *
 * 64 KiB = 128 packets on a 512-byte high-speed endpoint. The pipe is free to re-segment and the
 * device must accept any segmentation anyway, so this is not a protocol number — it is the unit of
 * *host-side* work. Each `transferOut` costs a renderer → USB-service round trip, so a small chunk
 * pays that latency more often, and with {@link UPLOAD_WINDOW} > 1 the only cost of a large one is
 * how coarsely the progress bar moves.
 *
 * Sweep it together with `UPLOAD_WINDOW`: the bytes the browser keeps queued at the endpoint are
 * their product (256 KiB today), which is what has to cover a device-side flush without the wire
 * going idle.
 */
export const DEFAULT_CHUNK_SIZE = 64 * 1024;

/**
 * How many `transferOut`s an upload keeps in flight at once.
 *
 * **The single biggest host-side lever, and the reason is latency, not bandwidth.** WebUSB hands a
 * transfer from the renderer process to the browser's USB service and back; with exactly one
 * outstanding, the wire is idle for that round trip between *every* chunk, and the device — which
 * NAKs anyway while it writes a staging half to the card — has nothing queued to absorb the moment
 * it comes back. A small window keeps the endpoint fed across both gaps.
 *
 * Small on purpose. Backpressure is what stops a 300 MB map from being read into the tab faster
 * than the card can take it ({@link BytePipe.write}'s doc), and a deep queue would defeat it; four
 * chunks is enough to cover a round trip and a flush, and no more. The window also bounds how far
 * the progress bar can run ahead of what the device has actually taken — see the loop in
 * {@link ProtocolClient.upload}, which only counts a chunk once its transfer has settled.
 */
export const UPLOAD_WINDOW = 4;

/**
 * How long to wait for a device answer before giving up.
 *
 * 15 s, matching the bounded status-wait the iOS app settled on after an unbounded one wedged the
 * transfer slot forever (trips epic #526). A device that has stopped answering must produce an
 * error, never a spinner that outlives the ride.
 */
export const DEFAULT_TIMEOUT_MS = 15_000;

/**
 * The base budget for a commit the caller has told us is **expensive**, via
 * {@link TransferOptions.commitBytes}.
 *
 * **This is opt-in, and deliberately so.** Almost every commit is cheap and bounded: a map's is a
 * close, an open, a 40-byte header read, a 4-byte write and a flush
 * (`Storage::map_upload_commit`), and a route's is smaller still — {@link DEFAULT_TIMEOUT_MS} is
 * already generous for those, and raising it globally would only mean a genuinely wedged device
 * takes four times longer to say so. The failure this exists for has exactly one instance.
 *
 * A **volume-set manifest** is a file under 2 KB whose commit re-opens and cross-checks every shard
 * header already on the card (`Storage::set_manifest_commit` → `set_shard_totals`,
 * `firmware/obc-fw-nrf54l/src/sd.rs`) — up to 32 directory lookups and header reads, behind a card
 * that may still be finishing the program cycle of the shard before it. Timing that out is not a
 * slow spinner, it is **data loss**: the client throws *and* `withTransferSlot` fires an `op = 3`
 * abort, which makes the device delete a set it may have just committed successfully.
 */
const COMMIT_TIMEOUT_BASE_MS = 60_000;

/**
 * Extra commit budget per megabyte the device has to re-read, capped by
 * {@link COMMIT_TIMEOUT_MAX_MS}.
 *
 * Deliberately loose: this bound exists to stop an infinite spinner, not to police the device's
 * timing.
 */
const COMMIT_TIMEOUT_PER_MB_MS = 200;

/** Ceiling on the commit wait, so a mis-announced length cannot produce an unbounded spinner. */
const COMMIT_TIMEOUT_MAX_MS = 10 * 60_000;

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
    /**
     * How many bytes the device has to **re-read** to commit this object — set this *only* where the
     * commit is more than a header check, because it is what buys the terminal wait a much larger
     * budget than {@link DEFAULT_TIMEOUT_MS}.
     *
     * One caller sets it today, and it is the one that would otherwise lose data: a volume-set
     * manifest is under 2 KB, but committing it cross-checks every shard already on the card, so its
     * wait has to be budgeted against the *set*. Left unset, an upload waits the ordinary timeout,
     * which is what keeps a wedged device quick to surface.
     */
    commitBytes?: number;
    /**
     * Called once the last byte has been handed to the transport, before the wait for the device's
     * verdict.
     *
     * The gap between those two is invisible from the outside — the bar is at 100 %, the rate is 0,
     * and nothing is moving — so a caller that shows progress wants to say what is happening.
     */
    onSent?: () => void;
}

/**
 * The wait budget for the terminal `transferResult` of an upload whose commit was declared expensive
 * ({@link TransferOptions.commitBytes}). Ordinary uploads do not come here — see
 * {@link COMMIT_TIMEOUT_BASE_MS} for why the scaling is opt-in.
 */
export function commitTimeoutMs(commitBytes: number): number {
    const megabytes = Math.ceil(Math.max(commitBytes, 0) / (1024 * 1024));
    return Math.min(COMMIT_TIMEOUT_BASE_MS + megabytes * COMMIT_TIMEOUT_PER_MB_MS, COMMIT_TIMEOUT_MAX_MS);
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
                sent = await this.pumpChunks(src, chunkSize, signal, check, (done) =>
                    onProgress?.(done, src.totalLen),
                );
            }
            if (sent !== src.totalLen) {
                throw new DeviceError(
                    "protocol",
                    `the upload source yielded ${sent} bytes but announced ${src.totalLen}.`,
                );
            }

            // Guarded, and the guard is load-bearing: this hook runs *inside* the transfer slot's
            // try, so a caller whose UI update threw would unwind into `withTransferSlot`'s catch
            // and fire an `op = 3` abort — against a transfer the device is at that moment
            // committing. That is the exact data-loss shape the commit budget above exists to
            // prevent, and a progress label is not worth it.
            try {
                options.onSent?.();
            } catch (cause) {
                console.warn("obc: an upload's onSent hook threw; ignoring it", cause);
            }
            // Only a caller that declared an expensive commit gets the scaled budget; everything else
            // keeps the ordinary one, so a wedged device still surfaces in seconds.
            const commitWaitMs =
                options.commitBytes === undefined ? undefined : commitTimeoutMs(options.commitBytes);
            const result = await this.awaitTransferResult(signal, "upload", commitWaitMs);
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
     * Cancelling an active shard already sends `op=abort`; this is the other edge: the worker or
     * host can fail after one shard committed and before the next descriptor opens.
     *
     * **Naming the set is what asks for the deletion.** An idle `op = 3` naming anything else is a
     * quiesce that leaves every staged file alone (interface spec §5 rule 5) — which is what makes
     * a refused shard retryable, and what this call is deliberately not.
     */
    async abandonMapSet(): Promise<void> {
        await this.withTransferSlot(
            () => ({ type: ObjectType.MapSet, objectId: SINGLETON_OBJECT_ID }),
            async () => {
                // **`mapSet`, not `mapShard`, and that is the whole disambiguation.** An `op = 3`
                // with nothing in flight now means two different things, and the descriptor's type
                // is what tells the device which: naming the *set* abandons it, naming anything else
                // is a pure quiesce that touches no stored state.
                //
                // It used to name a shard, which was indistinguishable from the abort the failure
                // path sends after a shard the device refused — so one refused shard deleted the
                // whole set, and the retry sealed a manifest over nothing.
                await this.sendDescriptor({
                    op: Op.Abort,
                    type: ObjectType.MapSet,
                    objectId: SINGLETON_OBJECT_ID,
                    totalLen: 0,
                    crc32: 0,
                });
                await this.awaitTransferResult(undefined, "set-abandon", undefined, true);
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
        // Anything the device said about a previous, already-closed exchange is stale by
        // definition; clearing it here keeps a late answer from correlating with this transfer.
        this.statuses.drain();
        try {
            return await body();
        } catch (cause) {
            // **Always, even when the device has already answered.** This used to be skipped once a
            // terminal `transferResult` had been consumed, on the reasoning that the device's gate
            // was already clear so an abort would name a transfer that no longer exists. That
            // reasoning was about the *gate*, and it missed what the handshake is actually for here
            // — which is also why the flag that tracked it is gone: nothing else read it.
            //
            // The bulk channel is unframed and unacknowledged, so this upload may have several
            // `transferOut`s queued at the endpoint — and neither end can recall them. WebUSB cannot
            // cancel a submitted transfer, and the device cannot tell a leftover from the next
            // object's opening bytes. If the retry's descriptor arms while they are still arriving,
            // they *are* its opening bytes and its whole-object CRC fails; the device's own
            // discard-while-idle is a race against the next descriptor, not a guarantee.
            //
            // `op = 3` is the one point where both ends are synchronised: §4.2 has the host stop and
            // wait, and the firmware answers an abort-with-nothing-armed by **draining the endpoint
            // and then confirming** (`TransferDisposition::AnswerIdleAbort`). So the abort is what
            // makes the retry an ordinary first attempt rather than a coin flip. It costs one
            // control round trip, on the failure path only.
            await this.sendQuiesceAbort(descriptorOf());
            await this.resetBulk();
            throw asDeviceError(cause);
        } finally {
            this.transferBusy = false;
        }
    }

    /**
     * The failure path's `op = 3`: get the device to empty its endpoint before we retry, and change
     * nothing else.
     *
     * **Never names a `mapSet`.** An idle abort naming the set is the device's signal to abandon it —
     * every staged file deleted — and this abort fires after *any* failed exchange, including a
     * single shard or the manifest refused on CRC, which the caller is about to re-send. So a
     * set-shaped descriptor is rewritten to a shard-shaped one before it goes out: same exchange
     * named, none of the abandonment. Giving up on a set is
     * {@link ProtocolClient.abandonMapSet}, and it is always an explicit decision by the caller.
     *
     * The rewritten id is a shard index the set may not have (a manifest's `objectId` is not a part
     * id at all). That is deliberate and harmless: an idle abort's descriptor selects *which
     * cleanup*, and the quiesce is the one that does none — the device never looks the part up.
     */
    private async sendQuiesceAbort(target: { type: ObjectType; objectId: number }): Promise<void> {
        const quiesce =
            target.type === ObjectType.MapSet ? { type: ObjectType.MapShard, objectId: target.objectId } : target;
        await this.sendAbort(quiesce);
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
     * Stream a source's chunks with up to {@link UPLOAD_WINDOW} `transferOut`s outstanding, and
     * answer with the bytes the transport actually took.
     *
     * **Why a window at all.** One outstanding transfer means the wire is idle for a renderer →
     * USB-service round trip between every chunk, *and* that the device has nothing queued to absorb
     * the moment it comes back from writing a staging half to the card. Both gaps are covered by
     * keeping a few chunks queued at the endpoint. The device needs no protocol change for this: the
     * bulk channel is unframed (spec principle #2) and the firmware reads it a packet at a time
     * regardless of how the host segmented it.
     *
     * **Backpressure survives.** The window is bounded, so a source is still pulled at the device's
     * pace rather than read whole into the tab — the property {@link BytePipe.write}'s doc rests on.
     * It is bounded in *chunks*, so the bytes in flight are `UPLOAD_WINDOW × chunkSize`.
     *
     * **Progress counts settled bytes only.** A queued transfer is not yet the device's, so
     * reporting on hand-off would run the bar up to 100% while a quarter-megabyte was still on the
     * wire — and would make a failure look like it happened after the bytes landed.
     */
    private async pumpChunks(
        src: ObjectSource,
        chunkSize: number,
        signal: AbortSignal | undefined,
        check: () => void,
        onProgress: (done: number) => void,
    ): Promise<number> {
        /** Handed to the transport and not yet settled, oldest first. */
        const queued: Array<{ promise: Promise<void>; length: number }> = [];
        let settled = 0;
        const retireOldest = async () => {
            const oldest = queued.shift();
            if (!oldest) return;
            await oldest.promise;
            settled += oldest.length;
            onProgress(settled);
        };
        try {
            for await (const chunk of src.chunks(chunkSize)) {
                check();
                const promise = this.link.bulk.write(chunk, signal);
                // **Observed at queue time, awaited at retire time.** A rejection is "unhandled"
                // from the microtask turn it happens in until *something* has attached a handler,
                // and with a window open the fourth chunk can reject while the first three are still
                // pending — an `unhandledrejection` in the console, over the top of the caller's real
                // error. This throwaway `.catch` is the handler; `retireOldest` still awaits the
                // original promise, so nothing is swallowed.
                void promise.catch(() => {});
                queued.push({ promise, length: chunk.length });
                if (queued.length >= UPLOAD_WINDOW) await retireOldest();
            }
            while (queued.length > 0) await retireOldest();
        } catch (cause) {
            // Wait for the rest of the window before unwinding, so the caller's error is not raced
            // by a later chunk's. It does **not** mean the endpoint is idle: on a cancel, `write`
            // rejects the caller while the `transferOut` stays on the wire (WebUSB cannot cancel a
            // submitted transfer), so `WebUsbPipe.writesInFlight` may still be non-zero afterwards.
            // That is fine and is the safe direction — `reset()` reads it precisely so it can *skip*
            // `clearHalt` on a half that still has a transfer on it.
            await Promise.allSettled(queued.map((entry) => entry.promise));
            throw cause;
        }
        return settled;
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
        //
        // The mailbox is drained when a transfer slot is *taken*, not when it is released, so a
        // result that lands late — the `aborted` confirming the previous exchange's quiesce, say —
        // is still sitting here when the next upload starts. It must be **consumed and thrown
        // away**, not skipped over: a predicate that merely fails to match leaves it queued, and
        // the very next unfiltered `take` in `awaitTransferResult` hands it back as this upload's
        // terminal result — the same spurious failure one step later, and as `aborted` rather than
        // `crcMismatch` nothing upstream retries it.
        for (;;) {
            const early = this.statuses.tryTake(isTransferResult);
            if (!early) return;
            if (early.status !== TransferStatus.Aborted) throw this.transferFailure(early, "upload");
            // A stale confirmation of an exchange that is already over. Dropped, and the loop goes
            // on in case a real reject is queued behind it.
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
            // A reject instead of an announce: the device never armed the transfer.
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

    /**
     * The terminal `transferResult` of an exchange.
     *
     * **Skips stale `aborted` results unless one is what was asked for**, and the reason is that the
     * mailbox is a plain FIFO: `take` shifts the head whatever it is. A quiesce abort whose ack
     * arrived after its 2 s wait gave up is still queued, so without this the *next* upload's wait
     * returns that `aborted` as its own verdict — a failure the device never issued, on a code
     * (`aborted`) that `sendAssembledSetFile` does not retry. `expectAborted` is for the two callers
     * that legitimately want one: the abort handshake itself, and `abandonMapSet`.
     *
     * The skip shares the caller's budget rather than restarting it, so a device that says nothing
     * useful still times out on schedule.
     */
    private async awaitTransferResult(
        signal: AbortSignal | undefined,
        what: string,
        timeoutMs?: number,
        expectAborted = false,
    ) {
        const deadline = Date.now() + (timeoutMs ?? this.timeoutMs);
        for (;;) {
            const remaining = Math.max(0, deadline - Date.now());
            const msg = await this.statuses.take(remaining, signal, `the ${what} result`);
            if (msg.msg !== "transferResult") {
                throw new DeviceError("protocol", `expected a transfer result, got a ${msg.msg}.`);
            }
            if (!expectAborted && msg.status === TransferStatus.Aborted) continue;
            return msg;
        }
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
