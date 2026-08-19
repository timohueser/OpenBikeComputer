/**
 * `BytePipe` — the transport seam the whole USB stack sits on (C3, issue #902).
 *
 * ## Where this sits
 *
 * The stack is four layers, and this file is the bottom one:
 *
 * ```text
 *   session.svelte.ts   reactive shell — what a Svelte component holds
 *   client.ts           protocol v4: LIST, STATUS, GET, PUT, REMOVE, CANCEL, ARM
 *   records.ts          FLAT_Store_Protocol.md §5.2's record framing: `record_length u16 · frame`
 *   pipe.ts             two byte pipes — WebUSB, nusb (`../desktop/usb.ts`) or loopback underneath
 * ```
 *
 * The bottom layer really is swappable in isolation — that is what `BytePipe` is for, and the
 * loopback and WebUSB implementations prove it.
 *
 * There is no barrel over the stack: every consumer imports the module it actually needs, which is
 * what keeps `loopback.ts` — a full simulated device the hosted bundle has no business carrying —
 * out of the shipped chunks, and `platform/bundle.test.ts` honest about what ships.
 *
 * `FLAT_Store_Protocol.md` §5.2 gives USB **two bulk endpoint pairs** carrying length-prefixed
 * records, and says packet boundaries carry no protocol meaning: a record may span packets. So the
 * pipe below stays what it always was — an ordered, unframed byte channel — and `records.ts` is the
 * one place that turns those bytes back into records. That split is what lets BLE (one CoC SDU is
 * one frame) and USB (a record may be five packets) sit under one client.
 *
 * Three implementations target this file:
 *
 * - `webusb.ts` — a Chromium `navigator.usb` bulk endpoint pair.
 * - `loopback.ts` — an in-memory pipe wired to a simulated device, so the whole stack can be built
 *   and tested before LM20 USB silicon exists.
 * - `../desktop/usb.ts` — Rust `nusb` behind Tauri commands, for the desktop app (D4 #909). It
 *   lives under `lib/desktop/` rather than here because it imports `@tauri-apps/api`, which the
 *   hosted bundle must never contain.
 *
 * ## The two properties that are easy to get wrong
 *
 * **A read is not a message.** {@link BytePipe.read} hands back whatever the transport delivered —
 * one byte, a full packet, several packets coalesced. Callers must accumulate until they have as
 * many bytes as the record's own length prefix announced. Assuming a read returns a whole logical
 * unit passes on a loopback that happens to write whole records and then fails on hardware, where a
 * 4,112-byte record arrives as nine 512-byte packets.
 *
 * **Cancellation has to reach the transport.** A `read` parked on an endpoint that will never
 * deliver — because the rider pulled the cable — is exactly the stuck spinner #902's acceptance
 * calls out. Every blocking call therefore takes an `AbortSignal`, and every implementation must
 * settle promptly when the device disappears rather than waiting on a transfer that can no longer
 * complete.
 */

/**
 * Why a pipe operation failed.
 *
 * - `closed` — the pipe is closed, or the device went away mid-operation (an unplug). Terminal:
 *   nothing on this pipe will work again.
 * - `aborted` — the caller's `AbortSignal` fired. The pipe itself is still usable, but the byte
 *   stream is now at an unknown offset — possibly inside a record — so a cancelled transfer must
 *   {@link BytePipe.reset} before another transfer uses the channel.
 * - `device-error` — the transport rejected the transfer (a stall, a babble, a driver error).
 * - `unsupported` — this browser has no WebUSB at all. Firefox and Safari take this path, and the
 *   desktop app is the answer for them (#894).
 */
export type PipeErrorCode = "closed" | "aborted" | "device-error" | "unsupported";

/** A transport-level failure. `cause` carries the underlying `DOMException` where there is one. */
export class PipeError extends Error {
    readonly code: PipeErrorCode;

    constructor(code: PipeErrorCode, message: string, options?: { cause?: unknown }) {
        super(message, options);
        this.name = "PipeError";
        this.code = code;
    }
}

/**
 * One direction-agnostic byte channel: reliable, ordered, and unframed.
 *
 * A `BytePipe` is full-duplex — `read` and `write` may be in flight at once — but neither
 * direction is expected to tolerate two concurrent calls of its own kind. The client serialises
 * them, which is also what §1 requires of the device ("the device serves exactly one `PUT` or
 * `GET`").
 */
export interface BytePipe {
    /** Diagnostics only, never a branch target: `"webusb"`, `"loopback"`, `"native"`. */
    readonly transport: string;

    /** False once {@link close} has run or the device has disappeared. */
    readonly open: boolean;

    /**
     * Wait for the next bytes to arrive.
     *
     * Resolves with **at least one and possibly many** bytes — never an empty array, which would be
     * indistinguishable from a spurious wakeup. Rejects with {@link PipeError} `closed` at end of
     * stream (including an unplug) and `aborted` if `signal` fires first.
     */
    read(signal?: AbortSignal): Promise<Uint8Array>;

    /**
     * Hand `bytes` to the transport, resolving only once it has taken them.
     *
     * That resolution *is* the backpressure: on WebUSB the promise settles when the device's
     * endpoint has drained the transfer, so a writer that keeps a *bounded* number of calls
     * outstanding cannot outrun the device — the card writes at 8.2 MB/s over sEMMC (#1158) and the
     * FAT layer above it takes a cut of that, so it is still the slower end of the cable. A writer
     * that fires and forgets defeats it entirely, which is why the upload loop retires an old
     * transfer for every new one it queues (`client.ts::pumpStream`) rather than queueing the object.
     *
     * **Concurrent writes are allowed on this call, in submission order.** They were not, before the
     * upload pipeline was windowed; an implementation that cannot preserve order between two
     * outstanding writes cannot back this interface.
     */
    write(bytes: Uint8Array, signal?: AbortSignal): Promise<void>;

    /**
     * Discard everything buffered or in flight and return the pipe to a known-empty state.
     *
     * A transfer that ends before its last record leaves the channel at an unknown offset, possibly
     * mid-record — over BLE the app closes and reopens the CoC. `reset` is that step for a pipe that
     * has no channel to reopen, so that the next transfer starts on a record boundary.
     *
     * *How* an implementation gets there differs, and the difference is not cosmetic: the loopback
     * drops its queued slices, D4's native pipe cancels every URB and drains the completions, and
     * `webusb.ts` can do neither — see its `reset`, which is the one place this contract is met by
     * argument rather than by force.
     */
    reset(): Promise<void>;

    /** Release the transport. Idempotent; pending reads and writes reject with `closed`. */
    close(): Promise<void>;
}

/**
 * The pair of channels one device speaks over, plus USB's one out-of-band read.
 *
 * `FLAT_Store_Protocol.md` §5 keeps the protocol on two channels: `control` carries §3's control
 * frames, `stream` carries §3.8's stream frames. Both are record-framed by `records.ts`; neither is
 * a message pipe, because §5.2 lets a record span USB packets.
 *
 * The channels are named after the spec rather than after the endpoint type they happen to use. All
 * four endpoints are bulk endpoints; calling one of them "bulk" said nothing and hid that the
 * stream pair is the one with the 4,096-byte payload rule on it.
 */
export interface DeviceLink {
    /** §3's control frames: requests out, responses in, one frame per record. */
    readonly control: BytePipe;
    /** §3.8's stream frames: a 16-byte frame and its payload, one frame per record. */
    readonly stream: BytePipe;

    /**
     * One EP0 vendor device-to-host request, recipient **interface** (§5.2.1).
     *
     * Present only on a transport that can issue one. WebUSB can (`controlTransferIn`); the loopback
     * models it; the desktop bridge cannot today, because the Rust side exposes no control-transfer
     * command — so it omits this rather than answering with a fabricated payload, and
     * {@link FlatStoreClient.deviceInfo} says the host cannot ask rather than inventing a version.
     *
     * The interface number is the transport's own fact — it claimed the interface — so it is not a
     * parameter here. Resolves with however many bytes the device returned, which §5.2.1 says may be
     * short of `length`.
     */
    vendorIn?(request: number, value: number, length: number, signal?: AbortSignal): Promise<Uint8Array>;

    /** Close both pipes. Idempotent. */
    close(): Promise<void>;
}

/** Throw `PipeError("aborted")` if `signal` has already fired — the cheap pre-flight check. */
export function throwIfAborted(signal: AbortSignal | undefined, what: string): void {
    if (signal?.aborted) throw abortedError(signal, what);
}

/** The canonical abort rejection, carrying the caller's own `reason` as `cause`. */
export function abortedError(signal: AbortSignal | undefined, what: string): PipeError {
    return new PipeError("aborted", `${what} was cancelled.`, { cause: signal?.reason });
}

/**
 * Race `promise` against `signal`, so a caller's cancel is observed even when the underlying
 * transfer cannot itself be cancelled.
 *
 * WebUSB has no way to cancel a submitted transfer, which is precisely why this exists: the
 * *caller* is released immediately and the transfer is left to settle on its own. Releasing the
 * caller is **not** the same as being rid of the transfer, and confusing the two is a real bug —
 * what happens to the one still on the endpoint is `webusb.ts`'s {@link BytePipe.reset} to explain.
 */
export function withAbort<T>(promise: Promise<T>, signal: AbortSignal | undefined, what: string): Promise<T> {
    if (!signal) return promise;
    throwIfAborted(signal, what);
    return new Promise<T>((resolve, reject) => {
        let settled = false;
        const onAbort = () => {
            if (settled) return;
            settled = true;
            reject(abortedError(signal, what));
        };
        signal.addEventListener("abort", onAbort, { once: true });
        promise.then(
            (value) => {
                signal.removeEventListener("abort", onAbort);
                if (settled) return; // the caller is long gone; the transfer's fate is the pipe's business
                settled = true;
                resolve(value);
            },
            (reason: unknown) => {
                signal.removeEventListener("abort", onAbort);
                if (settled) return;
                settled = true;
                reject(reason);
            },
        );
    });
}
