/**
 * The flat-store client: protocol v4's eight opcodes driven over a {@link DeviceLink}. One
 * implementation serves WebUSB, the desktop `nusb` bridge and an in-memory loopback, because
 * everything below it is two record channels and one optional EP0 read.
 *
 * A `RequestId` is chosen here and never reused: a response is correlated by it and by nothing else,
 * so a late answer to an abandoned request is dropped. Only one transfer runs at a time device-wide,
 * and the device is the authority on that, because the other transfer can be a phone's over BLE.
 * There are no unsolicited control frames: anything that wants to know the catalog moved re-lists.
 */

import { Crc32 } from "./crc32";
import { PipeError, throwIfAborted, type DeviceLink } from "./pipe";
import {
    DEVICE_INFO_MAX,
    GET_DEVICE_INFO,
    MAX_DEVICE_RECORD,
    MAX_HOST_CONTROL_RECORD,
    MAX_HOST_STREAM_RECORD,
    MAX_STREAM_PAYLOAD,
    RecordChannel,
    RecordError,
    decodeDeviceInfo,
    frameRecord,
    type DeviceInfo,
} from "./records";
import {
    Detail,
    ErrorCode,
    NO_OBJECT,
    Opcode,
    ResponseError,
    decodeResponse,
    encodeArmRequest,
    encodeCancelRequest,
    encodeFormatRequest,
    encodeGetRequest,
    encodeListRequest,
    encodePutRequest,
    encodeRemoveRequest,
    encodeStatusRequest,
    encodeStreamRecord,
    hex as hexBytes,
    opcodeName,
    refusalName,
    splitStreamRecord,
    toSafeNumber,
    type ArmResponse,
    type CatalogEntry,
    type GetResponse,
    type FormatResponse,
    type ListPage,
    type ObjectKind,
    type ObjectRef,
    type PutResponse,
    type Refusal,
    type Response,
    type StatusResponse,
} from "./protocol";

export type { DeviceInfo };

/** The destructive confirmation used when LIST cannot report a readable store identity. */
export const ZERO_STORE_ID = "00000000000000000000000000000000";

function mintStoreId(avoid: string): string {
    for (;;) {
        const bytes = new Uint8Array(16);
        globalThis.crypto.getRandomValues(bytes);
        const id = hexBytes(bytes);
        if (id !== ZERO_STORE_ID && id !== avoid) return id;
    }
}

/**
 * Why a device operation failed.
 *
 * The wire's fourteen refusal codes each get their own member, because a client's response to them
 * differs: `no-space` asks the rider to delete something, `busy` retries, `catalog-changed` restarts
 * a listing. The last five are this side of the wire, not the device's.
 */
export type DeviceErrorCode =
    | "link"
    | "timeout"
    | "aborted"
    | "protocol"
    | "unavailable"
    | "unsupported"
    | "invalid-frame"
    | "invalid-request"
    | "not-found"
    | "revision-conflict"
    | "no-space"
    | "checksum"
    | "media-io"
    | "busy"
    | "cancelled"
    | "rejected"
    | "internal"
    | "catalog-changed"
    | "read-only"
    | "device-error";

/** A failure at the protocol layer. `refusal` carries the device's own refusal body. */
export class DeviceError extends Error {
    readonly code: DeviceErrorCode;
    /** The wire refusal, when this error is one. Its `context` is code-scoped. */
    readonly refusal?: Refusal;

    constructor(code: DeviceErrorCode, message: string, options?: { cause?: unknown; refusal?: Refusal }) {
        super(message, options);
        this.name = "DeviceError";
        this.code = code;
        this.refusal = options?.refusal;
    }
}

/** True when LIST cannot name a store but FORMAT is still the intended recovery path. */
export function isFormatRecoveryState(cause: unknown): cause is DeviceError {
    return (
        cause instanceof DeviceError &&
        cause.code === "read-only" &&
        (cause.refusal?.detail === Detail.readOnly.unformatted ||
            cause.refusal?.detail === Detail.readOnly.catalogUnreadable)
    );
}

/**
 * Payload bytes handed to the transport per `write`, batched into whole stream records.
 *
 * A full stream record is 8,192 payload bytes and is not a tuning knob; this is how many of those go
 * into one `transferOut`. Sweep it with {@link UPLOAD_WINDOW}: their product is what the browser
 * keeps queued at the endpoint, which has to cover a device-side flush without the wire going idle.
 */
export const DEFAULT_BATCH_BYTES = 64 * 1024;

/**
 * How many batched writes an upload keeps in flight at once. A chosen value, and the lever is
 * latency rather than bandwidth: with exactly one outstanding, the wire is idle for a renderer to
 * USB-service round trip between every batch. Small on purpose — backpressure is what stops a 300 MB
 * map being read into the tab faster than the card can take it.
 */
export const UPLOAD_WINDOW = 4;

/**
 * How long to wait for a device answer before giving up. 15 s, a chosen value: a device that has
 * stopped answering must produce an error, never a spinner that outlives the ride. It bounds a round
 * trip, never a transfer, so a `PUT`'s clock starts when the last byte has been sent.
 */
export const DEFAULT_TIMEOUT_MS = 15_000;

/**
 * How long a cancel waits for the transfer's own `cancelled` response. A guess, and deliberately
 * short: the cancel is bilateral, so the allocation is released and the catalog is unchanged whether
 * or not this side sees the answer. The wait exists so the channels are quiet before they are reset.
 */
const CANCEL_ACK_TIMEOUT_MS = 2_000;

/**
 * How many times a paged listing restarts on `catalogChanged` before giving up. A guess: a device
 * whose catalog changes four times during one listing is one that something else is writing to
 * continuously, and looping forever would be worse than saying so.
 */
const LIST_RESTARTS = 4;

/** An object to upload, with its length and whole-payload CRC known before the first byte moves. */
export interface ObjectSource {
    readonly totalLen: number;
    readonly crc32: number;
    /** Yield the payload's bytes in order, in slices of at most `chunkSize`. */
    chunks(chunkSize: number): AsyncIterable<Uint8Array>;
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
 * A `Blob` — a fetched map, a picked file — without ever holding it twice. The CRC is needed before
 * the first byte streams and the payload cannot be re-derived from a suffix, so the blob is read
 * twice: once to fingerprint, once to send. For a 200 MB map the alternative is a second 200 MB
 * JavaScript buffer.
 */
export async function blobSource(
    blob: Blob,
    options: { signal?: AbortSignal; onProgress?: (done: number, total: number) => void } = {},
): Promise<ObjectSource> {
    const crc = new Crc32();
    let read = 0;
    for await (const chunk of streamChunks(blob.stream(), options.signal)) {
        crc.update(chunk);
        read += chunk.length;
        options.onProgress?.(read, blob.size);
    }
    return {
        totalLen: blob.size,
        crc32: crc.value(),
        async *chunks(chunkSize: number) {
            for await (const chunk of streamChunks(blob.stream(), options.signal)) {
                for (let at = 0; at < chunk.length; at += chunkSize) {
                    yield chunk.subarray(at, Math.min(at + chunkSize, chunk.length));
                }
            }
        },
    };
}

async function* streamChunks(
    stream: ReadableStream<Uint8Array>,
    signal?: AbortSignal,
): AsyncGenerator<Uint8Array> {
    const reader = stream.getReader();
    const onAbort = () => void reader.cancel(signal?.reason).catch(() => undefined);
    signal?.addEventListener("abort", onAbort, { once: true });
    try {
        signal?.throwIfAborted();
        for (;;) {
            const { done, value } = await reader.read();
            signal?.throwIfAborted();
            if (done) return;
            if (value.length) yield value;
        }
    } finally {
        signal?.removeEventListener("abort", onAbort);
        reader.releaseLock();
    }
}

/** Per-call knobs shared by both transfer directions. */
export interface TransferOptions {
    /** Cancel the transfer. A cancelled transfer sends `CANCEL` before it unwinds. */
    signal?: AbortSignal;
    /** Called as bytes move, for a progress bar. `done` and `total` are byte counts. */
    onProgress?: (done: number, total: number) => void;
    /** Override {@link DEFAULT_BATCH_BYTES} for one upload. Rounded up to whole stream records. */
    batchBytes?: number;
    /**
     * The payload length the caller already knows, used only to give a download's progress bar a
     * denominator. Without it a 300 MB map sits at zero for twenty minutes and then jumps. It never
     * decides when to stop reading or what to verify: that is always the device's own answer.
     */
    expectedLength?: number;
    /**
     * Called once the last byte has been handed to the transport, before the wait for the device's
     * verdict. That gap is invisible from the outside — the bar is at 100 %, nothing is moving — so
     * a caller that shows progress wants to say what is happening.
     */
    onSent?: () => void;
}

/** What a client asks a `PUT` to publish. The payload's length and CRC come from the source. */
export interface PutTarget {
    /** {@link NO_OBJECT} creates a new object; anything else replaces that one. */
    objectId?: bigint;
    /** The revision the device last reported for `objectId`. Omitted (zero) when creating. */
    expectedRevision?: bigint;
    kind: ObjectKind;
    /** Up to 48 UTF-8 bytes. The caller trims; this refuses a longer one. */
    displayName: string;
    /** Ask the same commit to leave the displaced revision `RETAINED`. */
}

/** A downloaded object: its bytes, and what the device said it served. */
export interface GetResult extends GetResponse {
    readonly bytes: Uint8Array;
}

/** A whole catalog: the identity prefix and every page's entries, concatenated. */
export interface Catalog {
    readonly storeId: string;
    /** The sequence every page of this listing agreed on. A movement changes it. */
    readonly commitSequence: bigint;
    readonly entries: readonly CatalogEntry[];
}

/** Options for a {@link FlatStoreClient}. */
export interface ClientOptions {
    /** Bound on every wait for a device answer. Defaults to {@link DEFAULT_TIMEOUT_MS}. */
    timeoutMs?: number;
}

/** One outstanding request, waiting for the response that echoes its `RequestId`. */
interface Pending {
    readonly opcode: number;
    settle(value: Response): void;
    fail(cause: unknown): void;
    /** Non-null once this request has an outcome — what a streaming upload polls between batches. */
    outcome: { ok: true; response: Response } | { ok: false; cause: unknown } | null;
    /**
     * True only when the device answered. Distinct from `outcome`: a caller's abort and a link
     * failure both settle a waiter without the device having said anything, and the transfer is then
     * still live on the far end holding the one transfer slot. Only this flag may skip the `CANCEL`.
     */
    answered: boolean;
    readonly promise: Promise<Response>;
}

/**
 * A connected device, speaking protocol v4 over two record channels. It starts a read loop on the
 * control channel immediately, so an answer that overtakes the last stream bytes — the two channels
 * are independent — is waiting for its request rather than lost.
 */
export class FlatStoreClient {
    private readonly link: DeviceLink;
    private readonly timeoutMs: number;
    private readonly control: RecordChannel;
    private readonly stream: RecordChannel;

    private readonly pending = new Map<number, Pending>();
    private nextRequestId = 1;

    /** Held from a `PUT`/`GET`'s request until its answer — the local half of one-at-a-time. */
    private liveTransferId: number | null = null;

    private closed = false;
    private linkFailure: DeviceError | null = null;
    private readonly readLoop: Promise<void>;

    constructor(link: DeviceLink, options: ClientOptions = {}) {
        this.link = link;
        this.timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
        this.control = new RecordChannel(link.control, MAX_HOST_CONTROL_RECORD, MAX_DEVICE_RECORD);
        this.stream = new RecordChannel(link.stream, MAX_HOST_STREAM_RECORD, MAX_DEVICE_RECORD);
        this.readLoop = this.pumpControl();
    }

    /** The `RequestId` of the transfer this client has outstanding, or `null`. Diagnostics. */
    get liveTransfer(): number | null {
        return this.liveTransferId;
    }

    /**
     * The three identity strings, over EP0. Not a protocol request: it sits below the record framing
     * and is readable the moment the interface is claimed. A transport that cannot issue an EP0
     * vendor request says so here, because a fabricated firmware revision would feed "an update is
     * available" a lie.
     */
    async deviceInfo(signal?: AbortSignal): Promise<DeviceInfo> {
        if (!this.link.vendorIn) {
            throw new DeviceError(
                "unavailable",
                "This host cannot read the device's firmware version: it has no way to issue a USB " +
                    "control request.",
            );
        }
        this.checkOpen();
        try {
            const payload = await this.link.vendorIn(GET_DEVICE_INFO, 0, DEVICE_INFO_MAX, signal);
            return decodeDeviceInfo(payload);
        } catch (cause) {
            throw asDeviceError(cause);
        }
    }

    /** One `LIST` page, exactly as asked. The paging loop is {@link list}. */
    async listPage(
        request: { kind?: ObjectKind | null; cursor?: { objectId: bigint; revision: bigint; commitSequence: bigint } },
        signal?: AbortSignal,
    ): Promise<ListPage> {
        const response = await this.exchange(
            Opcode.List,
            (id) => encodeListRequest(id, { kind: request.kind ?? null, cursor: request.cursor ?? null }),
            signal,
        );
        return (response as Extract<Response, { opcode: typeof Opcode.List }>).body;
    }

    /**
     * The whole catalog, paged with a `(ObjectId, Revision)` cursor.
     *
     * The cursor is the pair and the page resumes strictly after it, because an object may hold two
     * entries while a previous revision is retained; a cursor of `ObjectId` alone would skip the
     * head of an object whose retained revision ended the previous page.
     *
     * `catalogChanged` restarts the listing from the first page rather than resuming: the sequence
     * the client was told is the only thing that made the earlier pages consistent with each other.
     */
    async list(options: { kind?: ObjectKind; signal?: AbortSignal } = {}): Promise<Catalog> {
        for (let attempt = 0; ; attempt++) {
            try {
                return await this.listOnce(options.kind ?? null, options.signal);
            } catch (cause) {
                const stale = cause instanceof DeviceError && cause.code === "catalog-changed";
                if (!stale || attempt >= LIST_RESTARTS) throw cause;
            }
        }
    }

    private async listOnce(kind: ObjectKind | null, signal?: AbortSignal): Promise<Catalog> {
        const first = await this.listPage({ kind }, signal);
        const entries: CatalogEntry[] = [...first.entries];
        let page = first;
        while (page.more) {
            const last = page.entries[page.entries.length - 1];
            if (!last) {
                // The device promised a further page and gave no cursor to ask for it. There is no
                // legal next request, so this is not a retry.
                throw new DeviceError("protocol", "The device announced a further catalog page but sent no entries.");
            }
            page = await this.listPage(
                {
                    kind,
                    cursor: {
                        objectId: last.objectId,
                        revision: last.revision,
                        commitSequence: first.commitSequence,
                    },
                },
                signal,
            );
            entries.push(...page.entries);
        }
        return { storeId: first.storeId, commitSequence: first.commitSequence, entries };
    }

    /**
     * Is this object at this revision the catalog's head? The reconcile path for a replace whose
     * link broke. A create cannot be reconciled this way — the device assigned the id, and that
     * assignment was in the lost response — which is what {@link findCreated} is for.
     */
    async status(ref: ObjectRef, signal?: AbortSignal): Promise<StatusResponse> {
        const response = await this.exchange(Opcode.Status, (id) => encodeStatusRequest(id, ref), signal);
        return (response as Extract<Response, { opcode: typeof Opcode.Status }>).body;
    }

    /**
     * Reconcile a lost create against the catalog.
     *
     * The match is `(kind, payload length, payload CRC, display name)`, and the CRC is what makes it
     * sound: two routes of the same length and name are common, two with the same CRC are the same
     * bytes. A false negative costs one duplicate object the caller removes once it sees both.
     */
    async findCreated(
        want: { kind: ObjectKind; payloadLength: bigint; payloadCrc32: number; displayName: string },
        signal?: AbortSignal,
    ): Promise<CatalogEntry | null> {
        const catalog = await this.list({ kind: want.kind, signal });
        return (
            catalog.entries.find(
                (entry) =>
                    entry.kind === want.kind &&
                    entry.payloadLength === want.payloadLength &&
                    entry.payloadCrc32 === want.payloadCrc32 &&
                    entry.displayName === want.displayName,
            ) ?? null
        );
    }

    /**
     * The device streams the payload, then answers with what it served.
     *
     * The two channels are independent, so the answer may arrive before the last stream records have
     * been read off this side's endpoint. The loop below therefore keeps reading until it has the
     * length the answer named. Length and CRC are verified here; a mismatch throws and nothing is
     * handed back.
     */
    async get(ref: ObjectRef, options: TransferOptions = {}): Promise<GetResult> {
        const { signal, onProgress } = options;
        return this.withTransferSlot(async (requestId) => {
            const pending = this.open(requestId, Opcode.Get);
            await this.send(this.control, encodeGetRequest(requestId, ref), signal);

            /** Aborts the stream reader once nothing more is due — see the parked-read case below. */
            const done = new AbortController();
            const readSignal = anySignal(signal, done.signal);
            const chunks: Uint8Array[] = [];
            let got = 0;
            let expected: number | null = null;
            let nextOffset = 0n;

            void pending.promise.then(
                (response) => {
                    expected = toSafeNumber(
                        (response as Extract<Response, { opcode: typeof Opcode.Get }>).body.payloadLength,
                        "the served payload length",
                    );
                    // Everything is already here: the reader is parked on a record that will never
                    // come, and only this side can release it.
                    if (got >= expected) done.abort();
                },
                () => done.abort(),
            );

            const reading = (async () => {
                while (expected === null || got < expected) {
                    const record = await this.stream.next(readSignal);
                    const split = splitStreamRecord(record);
                    if (!split) {
                        throw new DeviceError("protocol", "The device sent a stream record that is not one (§3.8).");
                    }
                    // A frame bearing a `RequestId` that is not the live transfer's is discarded in
                    // silence. Late frames from an already-answered transfer are ordinary traffic.
                    if (split.frame.transferRequestId !== requestId) continue;
                    if (split.frame.offset !== nextOffset) {
                        throw new DeviceError(
                            "protocol",
                            `The device streamed offset ${split.frame.offset} where ${nextOffset} was due; ` +
                                "§3.8's frames are contiguous and ascending.",
                        );
                    }
                    chunks.push(split.payload.slice());
                    got += split.payload.length;
                    nextOffset += BigInt(split.payload.length);
                    onProgress?.(got, expected ?? options.expectedLength ?? got);
                }
            })();
            // The answer can refuse before a byte streams, in which case the reader below is aborted
            // and rejects with nobody awaiting it yet. This is that handler.
            void reading.catch(() => {});

            const answer = (await this.settle(pending, signal, `the ${opcodeName(Opcode.Get)} answer`)) as Extract<
                Response,
                { opcode: typeof Opcode.Get }
            >;
            try {
                await reading;
            } catch (cause) {
                // The reader is only ever aborted by this side once the payload is complete; any
                // other failure is the transfer's.
                if (!(cause instanceof PipeError && cause.code === "aborted" && done.signal.aborted)) throw cause;
            }

            const length = toSafeNumber(answer.body.payloadLength, "the served payload length");
            if (got !== length) {
                throw new DeviceError(
                    "protocol",
                    `The device streamed ${got} bytes for an object it says is ${length} bytes.`,
                );
            }
            const bytes = concat(chunks, got);
            const crc = Crc32.of(bytes);
            if (crc !== answer.body.payloadCrc32) {
                throw new DeviceError(
                    "checksum",
                    `The downloaded object failed its checksum (the device declared ${hex(answer.body.payloadCrc32)}, ` +
                        `these bytes are ${hex(crc)}). Nothing was kept; try again.`,
                );
            }
            onProgress?.(got, length);
            return { ...answer.body, bytes };
        }, options.signal);
    }

    /**
     * Announce, stream, and read back what the commit published.
     *
     * The payload streams immediately, without waiting for an acceptance: the cost of a refusal is
     * one round trip of wasted bytes rather than a round trip on every upload. What makes that safe
     * is the adapter obligation that the control frame reaches the engine before any stream frame
     * bearing the same `RequestId`. Between batches this side polls for an early answer, so a device
     * that refused on the first megabyte is not pushed the remaining 299.
     */
    async put(target: PutTarget, source: ObjectSource | Uint8Array, options: TransferOptions = {}): Promise<PutResponse> {
        const src = source instanceof Uint8Array ? bytesSource(source) : source;
        const { signal, onProgress } = options;
        const batch = wholeRecords(options.batchBytes ?? DEFAULT_BATCH_BYTES);
        return this.withTransferSlot(async (requestId) => {
            const pending = this.open(requestId, Opcode.Put);
            await this.send(
                this.control,
                encodePutRequest(requestId, {
                    objectId: target.objectId ?? NO_OBJECT,
                    expectedRevision: target.expectedRevision ?? 0n,
                    payloadLength: BigInt(src.totalLen),
                    payloadCrc32: src.crc32,
                    kind: target.kind,
                    displayName: target.displayName,
                }),
                signal,
            );

            onProgress?.(0, src.totalLen);
            const sent = await this.pumpStream(requestId, src, batch, pending, signal, (done) =>
                onProgress?.(done, src.totalLen),
            );
            if (sent !== src.totalLen) {
                throw new DeviceError(
                    "protocol",
                    `The upload source yielded ${sent} bytes but declared ${src.totalLen}.`,
                );
            }

            // Guarded, and the guard is load-bearing: this hook runs inside the transfer slot's
            // try, so a caller whose UI update threw would cancel a transfer mid-commit.
            try {
                options.onSent?.();
            } catch (cause) {
                console.warn("obc: an upload's onSent hook threw; ignoring it", cause);
            }

            const answer = (await this.settle(pending, signal, "the upload's answer")) as Extract<
                Response,
                { opcode: typeof Opcode.Put }
            >;
            if (answer.body.payloadLength !== BigInt(src.totalLen) || answer.body.payloadCrc32 !== src.crc32) {
                throw new DeviceError(
                    "protocol",
                    `The device committed ${answer.body.payloadLength} bytes / ${hex(answer.body.payloadCrc32)} ` +
                        `where ${src.totalLen} bytes / ${hex(src.crc32)} were sent.`,
                );
            }
            return answer.body;
        }, options.signal);
    }

    /** One commit; a retained previous revision goes with the head. */
    async remove(ref: ObjectRef, signal?: AbortSignal): Promise<bigint> {
        const response = await this.exchange(Opcode.Remove, (id) => encodeRemoveRequest(id, ref), signal);
        return (response as Extract<Response, { opcode: typeof Opcode.Remove }>).body.commitSequence;
    }

    /**
     * `true` when a transfer was dropped, `false` for "no such transfer".
     *
     * Bilateral and symmetric: the cancelled `PUT` or `GET` also receives its own `cancelled` error
     * response, so a caller that cancels its own transfer sees both. Either way the allocation is
     * released and the catalog is unchanged.
     */
    async cancel(transferRequestId: number, signal?: AbortSignal): Promise<boolean> {
        const response = await this.exchange(
            Opcode.Cancel,
            (id) => encodeCancelRequest(id, { transferRequestId }),
            signal,
            CANCEL_ACK_TIMEOUT_MS,
        );
        return (response as Extract<Response, { opcode: typeof Opcode.Cancel }>).body.cancelled;
    }

    /**
     * Make an uploaded update package the next boot.
     *
     * The device's current policy refuses this and answers `rejected`; that is a stated gap rather
     * than a bug in this call. Uploading never installs, and arming is a separate authenticated
     * decision precisely so that delivery and installation stay different.
     */
    async arm(ref: { objectId: bigint; expectedRevision: bigint }, signal?: AbortSignal): Promise<ArmResponse> {
        const response = await this.exchange(
            Opcode.Arm,
            (id) => encodeArmRequest(id, { packageObjectId: ref.objectId, expectedRevision: ref.expectedRevision }),
            signal,
        );
        return (response as Extract<Response, { opcode: typeof Opcode.Arm }>).body;
    }

    /**
     * Replace the card with a new, empty flat store and reboot the device.
     *
     * `expectedStoreId` is the identity LIST reported, or `null` on the recovery path where LIST
     * answered `readOnly/unformatted`. The replacement is minted from the host's CSPRNG; it is an
     * era identifier rather than a secret, but re-use would make stale object ids look live.
     */
    async format(
        expectedStoreId: string | null,
        options: { signal?: AbortSignal; replacementStoreId?: string } = {},
    ): Promise<FormatResponse> {
        const expected = expectedStoreId ?? ZERO_STORE_ID;
        const replacement = options.replacementStoreId ?? mintStoreId(expected);
        const response = await this.exchange(
            Opcode.Format,
            (id) => encodeFormatRequest(id, { expectedStoreId: expected, replacementStoreId: replacement }),
            options.signal,
        );
        const body = (response as Extract<Response, { opcode: typeof Opcode.Format }>).body;
        if (body.storeId !== replacement) {
            throw new DeviceError(
                "protocol",
                `The device formatted store ${body.storeId}, but this request minted ${replacement}.`,
            );
        }
        return body;
    }

    /** Close both channels and fail every waiter. Idempotent. */
    async close(): Promise<void> {
        this.closed = true;
        await this.link.close();
        await this.readLoop;
        this.failAll(new DeviceError("link", "The device link was closed."));
    }

    /**
     * The single reader on the control channel.
     *
     * It runs for the client's whole life, which is what lets a `CANCEL` go out mid-download and be
     * answered, and a `LIST` be served beside a live transfer. When the channel dies every waiter is
     * failed at once instead of being left to time out: a one-second error message rather than a
     * fifteen-second spinner.
     *
     * A record this build cannot read as a response is fatal to the channel, not skipped. There are
     * no unsolicited frames, so an unreadable answer means the two ends disagree about the wire.
     */
    private async pumpControl(): Promise<void> {
        for (;;) {
            let record: Uint8Array;
            try {
                record = await this.control.next();
            } catch (cause) {
                const closed = cause instanceof PipeError && cause.code === "closed";
                this.failAll(
                    closed && this.closed
                        ? new DeviceError("link", "The device link was closed.")
                        : new DeviceError("link", "The device disconnected.", { cause }),
                );
                return;
            }
            try {
                this.dispatch(decodeResponse(record));
            } catch (cause) {
                this.failAll(asDeviceError(cause));
                return;
            }
        }
    }

    private dispatch(answer: ReturnType<typeof decodeResponse>): void {
        const waiter = this.pending.get(answer.requestId);
        // An answer to a request nobody is waiting for is a late answer to one that was abandoned,
        // and there is nothing to do with it.
        if (!waiter) return;
        waiter.answered = true;
        if (!answer.ok) {
            waiter.fail(refusalError(answer.refusal, waiter.opcode));
            return;
        }
        if (answer.response.opcode !== waiter.opcode) {
            waiter.fail(
                new DeviceError(
                    "protocol",
                    `The device answered ${opcodeName(answer.response.opcode)} to a ` +
                        `${opcodeName(waiter.opcode)} request.`,
                ),
            );
            return;
        }
        waiter.settle(answer.response);
    }

    private failAll(error: DeviceError): void {
        this.linkFailure ??= error;
        for (const waiter of [...this.pending.values()]) waiter.fail(error);
        this.pending.clear();
    }

    /** One request, one answer — everything that is not a transfer. */
    private async exchange(
        opcode: number,
        encode: (requestId: number) => Uint8Array,
        signal?: AbortSignal,
        timeoutMs?: number,
    ): Promise<Response> {
        const requestId = this.mintRequestId();
        const pending = this.open(requestId, opcode);
        try {
            await this.send(this.control, encode(requestId), signal);
            return await this.settle(pending, signal, `the ${opcodeName(opcode)} answer`, timeoutMs);
        } finally {
            this.pending.delete(requestId);
        }
    }

    /**
     * Run a transfer holding the single slot, cancelling and resetting on any unhappy exit.
     *
     * A `PUT` this side walked away from is still live on the device, holding the engine against
     * every later request, and the `CANCEL` is what releases it. The channel reset then discards
     * whatever the abandoned transfer left buffered here, so the next transfer starts at a record
     * boundary rather than inside somebody else's payload.
     */
    private async withTransferSlot<T>(body: (requestId: number) => Promise<T>, signal?: AbortSignal): Promise<T> {
        if (this.liveTransferId !== null) {
            throw new DeviceError("busy", "Another transfer is already running. Wait for it to finish.");
        }
        this.checkOpen();
        if (signal?.aborted) throw new DeviceError("aborted", "The transfer was cancelled.", { cause: signal.reason });
        const requestId = this.mintRequestId();
        this.liveTransferId = requestId;
        try {
            return await body(requestId);
        } catch (cause) {
            await this.abandon(requestId);
            throw asDeviceError(cause);
        } finally {
            this.pending.delete(requestId);
            this.liveTransferId = null;
        }
    }

    /**
     * Best-effort `CANCEL`, then empty the stream channel of whatever the transfer left.
     *
     * The cancel is skipped only when the device has already answered, because the answer would then
     * be "no such transfer". Every other way out of a transfer needs it, and a cancel this side
     * merely decided on is exactly the case: the transfer is still live on the device, holding the
     * one slot against every later request.
     *
     * The channel reset is never skipped, because a transfer abandoned mid-record leaves this side's
     * reader inside somebody else's payload.
     */
    private async abandon(requestId: number): Promise<void> {
        if (this.closed || this.linkFailure) return;
        if (this.pending.get(requestId)?.answered === false) {
            try {
                // The signal bounds the write; the fourth argument only bounds the wait for an
                // answer. Without both, a device that is enumerated but hung — the endpoint NAKing
                // forever rather than failing — parks here and never releases `liveTransferId`.
                await this.cancel(requestId, AbortSignal.timeout(CANCEL_ACK_TIMEOUT_MS));
            } catch {
                // A device that is gone, one that never answers, and one that never even accepts
                // the write are all the same thing here. The reset below is the backstop either way.
            }
        }
        await this.stream.reset().catch(() => undefined);
    }

    /**
     * Stream a source's payload as records, with up to {@link UPLOAD_WINDOW} writes in flight.
     *
     * Records are exactly {@link MAX_STREAM_PAYLOAD} payload bytes except the last, because that is
     * what the device writes to the card in one go. Several are batched into one transport write, so
     * the window is measured in batches rather than in records.
     *
     * Progress counts settled bytes only. A queued transfer is not yet the device's, so reporting on
     * hand-off would run the bar to 100 % while a quarter-megabyte was still on the wire.
     */
    private async pumpStream(
        requestId: number,
        src: ObjectSource,
        batchBytes: number,
        pending: Pending,
        signal: AbortSignal | undefined,
        onProgress: (done: number) => void,
    ): Promise<number> {
        /** Handed to the transport and not yet settled, oldest first. */
        const queued: Array<{ promise: Promise<void>; payload: number }> = [];
        let settled = 0;
        let offset = 0n;
        const retireOldest = async () => {
            const oldest = queued.shift();
            if (!oldest) return;
            await oldest.promise;
            settled += oldest.payload;
            onProgress(settled);
        };
        /** The batch being assembled: whole records, back to back. */
        let batch: Uint8Array[] = [];
        let batchPayload = 0;
        const flush = async () => {
            if (batch.length === 0) return;
            const bytes = concat(batch, batch.reduce((n, part) => n + part.length, 0));
            const payload = batchPayload;
            batch = [];
            batchPayload = 0;
            const promise = this.link.stream.write(bytes, signal);
            // Observed at queue time, awaited at retire time. A rejection is unhandled from the
            // microtask turn it happens in until something attaches a handler, so this throwaway
            // `.catch` is that handler; `retireOldest` still awaits the original promise.
            void promise.catch(() => {});
            queued.push({ promise, payload });
            if (queued.length >= UPLOAD_WINDOW) await retireOldest();
        };

        try {
            for await (const chunk of src.chunks(MAX_STREAM_PAYLOAD)) {
                for (let at = 0; at < chunk.length; at += MAX_STREAM_PAYLOAD) {
                    this.checkUploadOpen(pending, signal);
                    const payload = chunk.subarray(at, Math.min(at + MAX_STREAM_PAYLOAD, chunk.length));
                    // Framed here rather than through `RecordChannel.send`, because several records
                    // go out in one transport write and the channel sends one at a time.
                    batch.push(frameRecord(encodeStreamRecord(requestId, offset, payload)));
                    batchPayload += payload.length;
                    offset += BigInt(payload.length);
                    if (batchPayload >= batchBytes) await flush();
                }
            }
            await flush();
            while (queued.length > 0) await retireOldest();
        } catch (cause) {
            // Wait for the rest of the window before unwinding, so the caller's error is not raced
            // by a later batch's. It does not mean the endpoint is idle.
            await Promise.allSettled(queued.map((entry) => entry.promise));
            throw cause;
        }
        return settled;
    }

    /**
     * May the upload keep pushing bytes? Two reasons it may not, and the second is easy to miss: the
     * caller cancelled, or the device has already answered. A refusal can arrive while these bytes
     * are queued — the price of streaming without an acceptance — so it is checked between records.
     */
    private checkUploadOpen(pending: Pending, signal?: AbortSignal): void {
        throwIfAborted(signal, "the upload");
        const outcome = pending.outcome;
        if (!outcome) return;
        if (!outcome.ok) throw outcome.cause;
        // A success before the last byte would mean the device committed a payload it has not seen.
        throw new DeviceError("protocol", "The device answered the upload before its payload had been sent.");
    }

    private mintRequestId(): number {
        // A `RequestId` is never reused after an answer: a terminated transfer can leave in-flight
        // stream frames that a reuse would absorb as its own. `0` is skipped as unanswerable.
        const id = this.nextRequestId;
        this.nextRequestId = this.nextRequestId >= 0xffffffff ? 1 : this.nextRequestId + 1;
        return id;
    }

    private open(requestId: number, opcode: number): Pending {
        let settle!: (value: Response) => void;
        let fail!: (cause: unknown) => void;
        const promise = new Promise<Response>((resolve, reject) => {
            settle = resolve;
            fail = reject;
        });
        // Nothing else attaches a handler until `settle()` awaits it, and a rejection that lands in
        // between would be reported as unhandled over the caller's own error.
        void promise.catch(() => {});
        const entry: Pending = {
            opcode,
            outcome: null,
            answered: false,
            promise,
            settle(response) {
                if (entry.outcome) return;
                entry.outcome = { ok: true, response };
                settle(response);
            },
            fail(cause) {
                if (entry.outcome) return;
                entry.outcome = { ok: false, cause };
                fail(cause);
            },
        };
        this.pending.set(requestId, entry);
        return entry;
    }

    /** Await one pending answer under a timeout and the caller's cancel. */
    private async settle(
        pending: Pending,
        signal: AbortSignal | undefined,
        what: string,
        timeoutMs?: number,
    ): Promise<Response> {
        const budget = timeoutMs ?? this.timeoutMs;
        if (pending.outcome) return pending.promise;
        throwIfAborted(signal, what);
        return new Promise<Response>((resolve, reject) => {
            const timer = setTimeout(() => {
                cleanup();
                pending.fail(new DeviceError("timeout", `The device did not answer ${what} within ${budget} ms.`));
            }, budget);
            const onAbort = () => {
                cleanup();
                pending.fail(
                    new DeviceError("aborted", `Waiting for ${what} was cancelled.`, { cause: signal?.reason }),
                );
            };
            const cleanup = () => {
                clearTimeout(timer);
                signal?.removeEventListener("abort", onAbort);
            };
            signal?.addEventListener("abort", onAbort, { once: true });
            pending.promise.then(
                (value) => {
                    cleanup();
                    resolve(value);
                },
                (cause: unknown) => {
                    cleanup();
                    reject(cause);
                },
            );
        });
    }

    private async send(channel: RecordChannel, frame: Uint8Array, signal?: AbortSignal): Promise<void> {
        this.checkOpen();
        try {
            await channel.send(frame, signal);
        } catch (cause) {
            throw asDeviceError(cause);
        }
    }

    private checkOpen(): void {
        if (this.linkFailure) throw this.linkFailure;
        if (this.closed) throw new DeviceError("link", "The device link is closed.");
    }
}

/** Map a wire refusal onto a caller-facing error, with the sentence that code deserves. */
export function refusalError(refusal: Refusal, opcode: number): DeviceError {
    const what = opcodeName(opcode);
    const named = refusalName(refusal);
    switch (refusal.code) {
        case ErrorCode.Unsupported:
            return new DeviceError(
                "unsupported",
                refusal.detail === Detail.unsupported.wireMajor
                    ? "This device speaks a different protocol version. Update the device firmware, or reload " +
                      "the page for a newer build."
                    : `The device does not support that ${refusal.detail === Detail.unsupported.kind ? "object kind" : "request"} (${named}).`,
                { refusal },
            );
        case ErrorCode.InvalidFrame:
            return new DeviceError("invalid-frame", `The device could not read this ${what} request (${named}).`, {
                refusal,
            });
        case ErrorCode.InvalidRequest:
            return new DeviceError("invalid-request", `The device refused this ${what} request (${named}).`, {
                refusal,
            });
        case ErrorCode.NotFound:
            return new DeviceError("not-found", "The device does not have that object.", { refusal });
        case ErrorCode.RevisionConflict:
            return new DeviceError(
                "revision-conflict",
                refusal.detail === Detail.revisionConflict.headAbsent
                    ? "That object is no longer on the device, so it cannot be replaced."
                    : `That object changed on the device (it is now at revision ${refusal.context}). ` +
                      "Refresh and try again.",
                { refusal },
            );
        case ErrorCode.NoSpace:
            return new DeviceError(
                "no-space",
                refusal.detail === Detail.noSpace.catalogFull
                    ? "The device's catalog is full. Delete something on the device and try again."
                    : `The card needs ${refusal.context} bytes for this and does not have them. ` +
                      "Delete something on the device and try again.",
                { refusal },
            );
        case ErrorCode.ChecksumFailure:
            return new DeviceError(
                "checksum",
                "The device rejected the upload: the payload did not match its checksum. Nothing was " +
                    "stored — try again.",
                { refusal },
            );
        case ErrorCode.MediaIo:
            return new DeviceError("media-io", `The device's card refused a ${named.split("/")[1] ?? "read"}.`, {
                refusal,
            });
        case ErrorCode.Busy:
            return new DeviceError(
                "busy",
                refusal.detail === Detail.busy.holds
                    ? "The device is holding too many objects open. Try again in a moment."
                    : "The device is already busy with another transfer.",
                { refusal },
            );
        case ErrorCode.Cancelled:
            return new DeviceError(
                "cancelled",
                refusal.detail === Detail.cancelled.byDevice
                    ? `The device stopped the ${what}.`
                    : `The ${what} was cancelled. Nothing was stored.`,
                { refusal },
            );
        case ErrorCode.Rejected:
            return new DeviceError(
                "rejected",
                `The device refused that object (${named}, detail ${refusal.detail}).`,
                { refusal },
            );
        case ErrorCode.Internal:
            return new DeviceError("internal", "The device hit a failure it could not classify.", { refusal });
        case ErrorCode.CatalogChanged:
            return new DeviceError(
                "catalog-changed",
                `The device's catalog changed while it was being listed (it is now at commit ${refusal.context}).`,
                { refusal },
            );
        case ErrorCode.ReadOnly:
            return new DeviceError(
                "read-only",
                refusal.detail === Detail.readOnly.unformatted
                    ? "The card in this device is not a flat store. Nothing can be read from it or written to it."
                    : "The device's card is read-only.",
                { refusal },
            );
        default:
            // An unknown code is a failure that cannot be classified, never a success.
            return new DeviceError("device-error", `The device answered the ${what} with ${named}.`, { refusal });
    }
}

/** Normalise a channel-level or unknown failure into a {@link DeviceError}. */
export function asDeviceError(cause: unknown): DeviceError {
    if (cause instanceof DeviceError) return cause;
    if (cause instanceof PipeError) {
        if (cause.code === "aborted") return new DeviceError("aborted", "The transfer was cancelled.", { cause });
        if (cause.code === "closed") return new DeviceError("link", "The device disconnected.", { cause });
        return new DeviceError("device-error", cause.message, { cause });
    }
    if (cause instanceof RecordError || cause instanceof ResponseError) {
        return new DeviceError("protocol", cause.message, { cause });
    }
    return new DeviceError("device-error", cause instanceof Error ? cause.message : String(cause), { cause });
}

/** Round a batch size up to whole stream records, so a batch never ends mid-record. */
function wholeRecords(bytes: number): number {
    return Math.max(MAX_STREAM_PAYLOAD, Math.ceil(bytes / MAX_STREAM_PAYLOAD) * MAX_STREAM_PAYLOAD);
}

function concat(parts: readonly Uint8Array[], total: number): Uint8Array {
    const out = new Uint8Array(total);
    let at = 0;
    for (const part of parts) {
        out.set(part, at);
        at += part.length;
    }
    return out;
}

/** One signal that fires when either of two do. `AbortSignal.any` is not in every target yet. */
function anySignal(a: AbortSignal | undefined, b: AbortSignal): AbortSignal {
    if (!a) return b;
    const controller = new AbortController();
    const forward = (reason: unknown) => controller.abort(reason);
    if (a.aborted) forward(a.reason);
    else a.addEventListener("abort", () => forward(a.reason), { once: true });
    if (b.aborted) forward(b.reason);
    else b.addEventListener("abort", () => forward(b.reason), { once: true });
    return controller.signal;
}

function hex(v: number): string {
    return `0x${v.toString(16).toUpperCase().padStart(8, "0")}`;
}
