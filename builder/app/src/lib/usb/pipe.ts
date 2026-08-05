/**
 * `BytePipe` — the transport seam the whole USB stack sits on (C3, issue #902).
 *
 * ## Where this sits
 *
 * The stack is four layers, and this file is the bottom one:
 *
 * ```text
 *   session.svelte.ts   reactive shell — what a Svelte component holds
 *   client.ts           the object model: identity, lists, transfers, commands
 *   transport.ts        the one byte USB adds: which control characteristic a frame is
 *   pipe.ts             two byte pipes — WebUSB, nusb (`../desktop/usb.ts`) or loopback underneath
 * ```
 *
 * The bottom layer really is swappable in isolation — that is what `BytePipe` is for, and the
 * loopback and WebUSB implementations prove it. `transport.ts` is a weaker seam: it owns the frame
 * *encoding* alone, but the assumption that control messages carry a leading selector is shared with
 * `client.ts`'s dispatch. Its header says exactly what moves if #889 ratifies something else.
 *
 * There is no barrel over the stack: every consumer imports the module it actually needs, which is
 * what keeps `loopback.ts` — a full simulated device the hosted bundle has no business carrying —
 * out of the shipped chunks, and `platform/bundle.test.ts` honest about what ships.
 *
 * The interface spec's principle #2 is that the bulk channel is a **raw byte pipe with no
 * per-chunk framing**: a control-plane descriptor announces the transfer, the channel carries
 * exactly the object's payload bytes, and one whole-object CRC-32 is verified at commit. That is
 * why a USB bulk endpoint slots in under the same object model as BLE's L2CAP CoC — the layer
 * above never learns which one it is talking to.
 *
 * So this file deliberately knows nothing about USB. Three implementations target it:
 *
 * - `webusb.ts` — a Chromium `navigator.usb` bulk endpoint pair (this issue).
 * - `loopback.ts` — an in-memory pipe wired to a simulated device, so C4 (#903), C5 (#904) and the
 *   whole desktop path can be built and tested before LM20 USB silicon exists (this issue).
 * - `../desktop/usb.ts` — Rust `nusb` behind Tauri commands, for the desktop app (D4 #909). It
 *   lives under `lib/desktop/` rather than here because it imports `@tauri-apps/api`, which the
 *   hosted bundle must never contain.
 *
 * ## The two properties that are easy to get wrong
 *
 * **A read is not a message.** {@link BytePipe.read} hands back whatever the transport delivered —
 * one byte, a full packet, several packets coalesced. Callers must accumulate until they have as
 * many bytes as the descriptor announced. Assuming a read returns a whole logical unit passes on a
 * loopback that happens to write whole objects and then fails on hardware, where a 62 KB object
 * arrives as a thousand 64-byte packets.
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
 *   stream is now at an unknown offset, so a cancelled transfer must {@link BytePipe.reset} before
 *   the pipe is handed to another descriptor (interface spec §4.1).
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
 * direction is expected to tolerate two concurrent calls of its own kind. The protocol client
 * serialises them, which is also what interface spec §4.1 requires of the object layer ("at most
 * one transfer is in flight at a time").
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
     * endpoint has drained the transfer, so a writer that awaits every call cannot outrun a device
     * whose SD card is the real bottleneck (high-hundreds of KB/s — #889). A writer that fires and
     * forgets defeats it, so callers await.
     */
    write(bytes: Uint8Array, signal?: AbortSignal): Promise<void>;

    /**
     * Discard everything buffered or in flight and return the pipe to a known-empty state.
     *
     * The spec's rule, §4.1: an exchange that does not reach its correlated close leaves the
     * channel at an unknown offset and is "not reusable" — over BLE the app closes and reopens the
     * CoC. `reset` is that step for a pipe that has no channel to reopen, so that the next
     * descriptor starts clean.
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
 * The pair of channels one device speaks over.
 *
 * BLE splits the protocol across two planes — small typed control state on GATT, bulk bytes on the
 * CoC (spec principle #1) — and USB keeps that split rather than flattening it: `control` carries
 * one message per operation, `bulk` carries raw object payload. Nothing large ever crosses
 * `control`, so a device can serve both from one interface without either starving the other.
 */
export interface DeviceLink {
    /** Message-oriented: one write is one control frame, one read is one control frame. */
    readonly control: BytePipe;
    /** The unframed object stream — BLE's CoC, byte for byte. */
    readonly bulk: BytePipe;
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
