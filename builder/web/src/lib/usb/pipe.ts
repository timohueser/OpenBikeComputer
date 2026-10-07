/**
 * The byte-channel seam under the USB stack: `records.ts` turns these bytes into records and
 * `client.ts` speaks the protocol above them. WebUSB, the desktop `nusb` bridge and the loopback
 * device implement it.
 *
 * There is no barrel over the stack. Every consumer imports the module it needs, which keeps the
 * simulated `loopback.ts` device out of the hosted bundle.
 */

/**
 * Why a pipe operation failed.
 *
 * - `closed` — the pipe is closed or the device went away. Terminal.
 * - `aborted` — the caller's `AbortSignal` fired. The byte stream is now at an unknown offset, so
 *   the pipe needs {@link BytePipe.reset} before another transfer uses it.
 * - `device-error` — the transport rejected the transfer.
 * - `unsupported` — this browser has no WebUSB.
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
 * One direction-agnostic byte channel: reliable, ordered and unframed.
 *
 * A pipe is full-duplex, but no direction tolerates two concurrent calls of its own kind. The
 * client serialises them.
 */
export interface BytePipe {
    /** Diagnostics only, never a branch target: `"webusb"`, `"loopback"`, `"native"`. */
    readonly transport: string;

    /** False once {@link close} has run or the device has disappeared. */
    readonly open: boolean;

    /**
     * Wait for the next bytes to arrive.
     *
     * A read is not a message: it resolves with at least one byte, but a record can span many
     * reads. The caller must accumulate until it has the count the record's length prefix gave.
     */
    read(signal?: AbortSignal): Promise<Uint8Array>;

    /**
     * Hand `bytes` to the transport, resolving only once it has taken them.
     *
     * That resolution is the backpressure: a writer that keeps a bounded number of calls
     * outstanding cannot outrun the device, and a writer that fires and forgets defeats it.
     * Concurrent writes are allowed and keep submission order.
     */
    write(bytes: Uint8Array, signal?: AbortSignal): Promise<void>;

    /**
     * Discard everything buffered or in flight, so that the next transfer starts on a record
     * boundary. A cancelled transfer can stop mid-record.
     */
    reset(): Promise<void>;

    /** Release the transport. Idempotent; pending reads and writes reject with `closed`. */
    close(): Promise<void>;
}

/**
 * The pair of channels one device speaks over, plus USB's one out-of-band read.
 *
 * `control` carries control frames and `stream` carries stream frames. Both are record-framed by
 * `records.ts`; neither is a message pipe.
 */
export interface DeviceLink {
    /** Control frames: requests out, responses in, one frame per record. */
    readonly control: BytePipe;
    /** Stream frames: a 16-byte frame and its payload, one frame per record. */
    readonly stream: BytePipe;

    /**
     * One EP0 vendor device-to-host request, recipient interface.
     *
     * Present only on a transport that can issue one. The desktop bridge exposes no control-transfer
     * command, so it omits this rather than answering with a fabricated payload. Resolves with
     * however many bytes the device returned, which can be short of `length`.
     */
    vendorIn?(request: number, value: number, length: number, signal?: AbortSignal): Promise<Uint8Array>;

    /** Close both pipes. Idempotent. */
    close(): Promise<void>;
}

/** Throw `PipeError("aborted")` if `signal` has already fired. */
export function throwIfAborted(signal: AbortSignal | undefined, what: string): void {
    if (signal?.aborted) throw abortedError(signal, what);
}

/** The canonical abort rejection, carrying the caller's own `reason` as `cause`. */
export function abortedError(signal: AbortSignal | undefined, what: string): PipeError {
    return new PipeError("aborted", `${what} was cancelled.`, { cause: signal?.reason });
}

/**
 * Race `promise` against `signal`.
 *
 * WebUSB cannot cancel a submitted transfer. This releases the caller immediately and leaves the
 * transfer to settle on its own; getting rid of the transfer is `webusb.ts`'s `reset`.
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
                if (settled) return;
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
