/**
 * The native USB transport (D4, #909): `nusb` behind C3's {@link BytePipe} seam.
 *
 * Tauri's webview has no WebUSB — WKWebView, WebView2 and WebKitGTK all lack it — so this tier
 * drives USB from Rust. #894 turns that into the point rather than the cost: it is the only
 * *universal* USB path, and the answer for every Safari and Firefox user the hosted site cannot
 * reach.
 *
 * **Nothing about the protocol lives here.** `lib/usb/` is the object model, the descriptors, the
 * whole-object CRC and the client, written once and validated against `specs/vectors/`; this
 * file supplies the two byte pipes underneath it and a watcher with the same three methods
 * `WebUsbWatcher` has. `ProtocolClient` and `DeviceLink` are used unchanged, and they cannot tell
 * which transport they got — which was C3's whole design claim, and this is the thing that either
 * makes it true or exposes it.
 *
 * ## Why it lives in `lib/desktop/` and not beside `webusb.ts`
 *
 * Because it imports `@tauri-apps/api`. `lib/desktop/` is the folder that already means "the Tauri
 * wire, reachable only from the desktop host", and `platform/bundle.test.ts` asserts nothing in it
 * — nor the Tauri API itself — reaches the hosted bundle. Putting a Tauri importer inside
 * `lib/usb/` would put that guard one careless re-export away from failing.
 *
 * ## What is different from the browser, and what deliberately is not
 *
 * - **Cancellation reaches the transport.** WebUSB cannot cancel a submitted `transferIn`, so C3's
 *   pipe releases the caller and lets the transfer settle into nothing. nusb *can* cancel, and here
 *   it must: after an abort the device stops sending by design, so an orphaned read would hold the
 *   endpoint forever. Every blocking call therefore also tells the backend to cancel the URB.
 * - **No chooser, no permission prompt.** `requestDevice()` is "look again now", not a dialog. The
 *   session's shape is identical anyway, so the Connect button and the auto-detect path both keep
 *   working without a single UI branch on which transport is underneath.
 * - **Reads are still not messages.** The bulk pipe hands back whatever one transfer delivered, and
 *   the client accumulates to the announced length. Nothing about that changes.
 */

import { PipeError, withAbort, type BytePipe, type DeviceLink } from "../usb/pipe";
import { ProtocolClient, type ClientOptions, type ObjectSource, type SendHooks } from "../usb/client";
import type { DeviceState, DeviceWatcher } from "../usb/session";
import { Channel } from "@tauri-apps/api/core";
import {
    desktop,
    type UsbDeviceSummary,
    type UsbEvent,
    type UsbLinkInfo,
    type UsbPlane,
    type UsbSendProgress,
} from "./invoke";

// --- the pipe -----------------------------------------------------------------

/** One endpoint pair, as a byte pipe over the Tauri commands in `apps/obc-desktop/src/usb/`. */
class NativePipe implements BytePipe {
    readonly transport = "native";

    private failure: PipeError | null = null;
    private closedByUs = false;
    /** Rejectors for {@link dead}, released when the pipe closes or fails. */
    private readonly mourners: Array<(error: PipeError) => void> = [];

    constructor(
        readonly kind: UsbPlane,
        private readonly handle: number,
        readonly packetSize: number,
    ) {}

    get open(): boolean {
        return !this.failure && !this.closedByUs;
    }

    async read(signal?: AbortSignal): Promise<Uint8Array> {
        this.check();
        // The backend cancels the URB, so the abort is not merely the caller walking away: the
        // transfer is really gone, and the endpoint is free for the reset that always follows.
        const release = this.cancelOnAbort(signal, "in");
        try {
            const body = await Promise.race([
                withAbort(desktop.usbRead(this.handle, this.kind), signal, "the read", () => undefined),
                this.dead(),
            ]);
            // Never empty: the backend absorbs zero-length packets, which are USB-level markers
            // (the object terminator #889 added) rather than data a caller could interpret.
            return new Uint8Array(body);
        } catch (cause) {
            throw this.asPipeError(cause, "read");
        } finally {
            release();
        }
    }

    async write(bytes: Uint8Array, signal?: AbortSignal): Promise<void> {
        this.check();
        // A control frame is one transfer, so it has to fit in one packet: at exactly the packet
        // size the device could not tell the frame had ended without a zero-length packet. Same
        // rule, same reason, as the WebUSB pipe and the firmware's own `const _: () = assert!`.
        if (this.kind === "control" && bytes.length >= this.packetSize) {
            throw new PipeError(
                "device-error",
                `a ${bytes.length}-byte control frame does not fit the ${this.packetSize}-byte endpoint.`,
            );
        }
        const release = this.cancelOnAbort(signal, "out");
        try {
            await Promise.race([
                withAbort(desktop.usbWrite(this.handle, this.kind, bytes), signal, "the write", () => undefined),
                this.dead(),
            ]);
        } catch (cause) {
            throw this.asPipeError(cause, "write");
        } finally {
            release();
        }
    }

    /**
     * Cancel everything in flight and clear both halves of the pair.
     *
     * Spec §4.1: an exchange that does not reach its correlated close leaves the channel at an
     * unknown offset. Here that is a real cancel plus `CLEAR_FEATURE(ENDPOINT_HALT)`, not the
     * best-effort `clearHalt` the browser can manage — so a cancelled download's tail is
     * genuinely discarded rather than raced against.
     */
    async reset(): Promise<void> {
        this.check();
        try {
            await desktop.usbReset(this.handle, this.kind);
        } catch (cause) {
            // A pipe whose device has already gone has nothing left to reset, and the caller's
            // original error is the interesting one.
            if (this.failure) return;
            throw this.asPipeError(cause, "reset");
        }
    }

    async close(): Promise<void> {
        this.closedByUs = true;
        this.bury(new PipeError("closed", "The device link is closed."));
    }

    /** Mark the pipe dead and fail everything waiting on it — what an unplug calls. */
    fail(error: PipeError): void {
        this.failure ??= error;
        this.bury(error);
    }

    /** Tell the backend to cancel this direction when `signal` fires. Returns the unsubscriber. */
    private cancelOnAbort(signal: AbortSignal | undefined, dir: "in" | "out"): () => void {
        if (!signal) return () => undefined;
        const onAbort = () => void desktop.usbCancel(this.handle, this.kind, dir).catch(() => undefined);
        signal.addEventListener("abort", onAbort, { once: true });
        return () => signal.removeEventListener("abort", onAbort);
    }

    /**
     * A promise that rejects when this pipe closes or fails.
     *
     * Raced against every call so an unplug settles the caller from the *event*, deterministically,
     * rather than from whichever of the OS transfer error and the hot-plug notification wins.
     */
    private dead(): Promise<never> {
        return new Promise<never>((_, reject) => {
            if (this.failure) reject(this.failure);
            else if (this.closedByUs) reject(new PipeError("closed", "The device link is closed."));
            else this.mourners.push(reject);
        });
    }

    private bury(error: PipeError): void {
        while (this.mourners.length) this.mourners.shift()?.(error);
    }

    private check(): void {
        if (this.failure) throw this.failure;
        if (this.closedByUs) throw new PipeError("closed", "The device link is closed.");
    }

    private asPipeError(cause: unknown, what: string): PipeError {
        if (this.failure) return this.failure;
        const error = asPipeError(cause, what);
        // A transport-level `closed` is terminal for this pipe, exactly as an unplug is: recording
        // it means the next call fails immediately instead of making another doomed round trip.
        if (error.code === "closed") this.fail(error);
        return error;
    }
}

/**
 * Translate a rejected Tauri command into a {@link PipeError}.
 *
 * The backend rejects with its own `{ code, message }` — deliberately in `PipeError`'s vocabulary,
 * so the mapping is a lookup rather than string-matching an OS error. Anything else (an IPC failure,
 * a command that isn't registered) is a `device-error`, which is honest: something below the
 * protocol broke.
 */
function asPipeError(cause: unknown, what: string): PipeError {
    if (cause instanceof PipeError) return cause;
    const fault = cause as { code?: unknown; message?: unknown } | null;
    const code = fault?.code;
    const message = typeof fault?.message === "string" ? fault.message : String(cause);
    if (code === "closed" || code === "aborted" || code === "device-error") {
        return new PipeError(code, message, { cause });
    }
    return new PipeError("device-error", `The ${what} failed: ${message}`, { cause });
}

// --- the link -----------------------------------------------------------------

/** A claimed device, as a {@link DeviceLink}. */
export interface NativeLink extends DeviceLink {
    readonly info: UsbLinkInfo;
    /** Fail both pipes at once — what a `disconnected` event calls. */
    disconnected(): void;
}

/** Open and claim a device by the backend's opaque id, returning its two pipes. */
export async function openNativeLink(deviceId: string): Promise<NativeLink> {
    let info: UsbLinkInfo;
    try {
        info = await desktop.usbOpen(deviceId);
    } catch (cause) {
        throw asPipeError(cause, "connection");
    }
    const control = new NativePipe("control", info.handle, info.controlPacketSize);
    const bulk = new NativePipe("bulk", info.handle, info.bulkPacketSize);
    return {
        info,
        control,
        bulk,
        disconnected() {
            const error = new PipeError("closed", "The device was unplugged.");
            control.fail(error);
            bulk.fail(error);
        },
        async close() {
            await control.close();
            await bulk.close();
            try {
                await desktop.usbClose(info.handle);
            } catch {
                // Closing a link whose device is already gone is the normal unplug path.
            }
        },
    };
}

// --- the bulk plane, by file path ---------------------------------------------

/**
 * An {@link ObjectSource} whose bytes never enter this process.
 *
 * #894's plane split, made concrete: the control plane (a 12-byte descriptor, a status envelope)
 * rides the IPC because it is tiny and the protocol that produces it is TypeScript; the object's
 * bytes go disk → endpoint inside Rust, because a country map is hundreds of megabytes and pushing
 * it through the webview so the webview could hand it straight back is theatre.
 *
 * The file is still read **twice** — once to fingerprint, once to send — for the same reason
 * `blobSource` reads a `Blob` twice: §4.2 announces the whole-object CRC-32 before the first byte
 * moves, and a checksum of bytes you have not seen does not exist. Both passes happen in Rust.
 *
 * The path must be one the backend will stream (its maps folder or its cache — see
 * `usb::sendable_path`); anything else is refused there rather than trusted here.
 */
export async function nativeFileSource(handle: number, path: string): Promise<ObjectSource> {
    const digest = await desktop.usbFileDigest(path);
    return {
        totalLen: digest.len,
        crc32: digest.crc32,
        // Unreachable: `ProtocolClient.upload` prefers `sendTo` whenever a source has one. It
        // throws rather than quietly reading the file over the IPC, because a silent fallback to
        // the thing this class exists to avoid is worse than a loud failure.
        // eslint-disable-next-line require-yield
        async *chunks(): AsyncGenerator<Uint8Array> {
            throw new Error(`${path} streams natively; its bytes are not available to the page.`);
        },
        sendTo: (_pipe, hooks) => sendNativeFile(handle, path, hooks),
    };
}

/**
 * Stream a file into the bulk endpoint, honouring the hooks the chunk loop would have honoured.
 *
 * The progress channel is doing two jobs. It moves the bar, and it is the *only* moment at which
 * this side can notice that the device rejected the descriptor — so `check()` runs on every report
 * and, when it throws, the send is cancelled at the transport and the device's reason is what
 * surfaces. Without that, a device answering "storage full" after the first megabyte would still be
 * pushed the remaining 299.
 */
async function sendNativeFile(handle: number, path: string, hooks: SendHooks): Promise<number> {
    // Before anything moves: a transfer already cancelled, or already rejected, must not start.
    hooks.check();

    const cancel = () => void desktop.usbCancel(handle, "bulk", "out").catch(() => undefined);
    /** The reason the send was stopped from this side, kept so it outranks the cancel it caused. */
    let stopped: unknown = null;

    const progress = new Channel<UsbSendProgress>();
    progress.onmessage = (report) => {
        hooks.onProgress?.(report.sent);
        if (stopped) return;
        try {
            hooks.check();
        } catch (cause) {
            stopped = cause;
            cancel();
        }
    };
    const onAbort = () => cancel();
    hooks.signal?.addEventListener("abort", onAbort, { once: true });
    try {
        return await desktop.usbSendFile(handle, path, progress);
    } catch (cause) {
        // "The device said storage-full" is the useful sentence; "the transfer was cancelled" is
        // merely how this side reacted to it.
        if (stopped) throw stopped;
        throw asPipeError(cause, "transfer");
    } finally {
        hooks.signal?.removeEventListener("abort", onAbort);
    }
}

// --- discovery ----------------------------------------------------------------

export interface NativeWatcherOptions extends ClientOptions {
    /** Injected by tests; defaults to the real Tauri bridge. */
    bridge?: Bridge;
}

/** The slice of the Tauri bridge discovery uses, named so a test can drive the real code paths. */
export interface Bridge {
    usbWatch(onEvent: Channel<UsbEvent>): Promise<UsbDeviceSummary[]>;
    usbList(): Promise<UsbDeviceSummary[]>;
}

/**
 * Finds the device, keeps up with plugging and unplugging, and owns the {@link ProtocolClient}.
 *
 * The same three-method shape as `WebUsbWatcher`, so `WatchedDeviceSession` wraps either one and
 * the UI above never learns which. Subscribers get an immutable snapshot per change.
 */
export class NativeWatcher implements DeviceWatcher {
    private readonly options: NativeWatcherOptions;
    private readonly bridge: Bridge;
    private readonly listeners = new Set<(state: DeviceState) => void>();

    private link: NativeLink | null = null;
    private state: DeviceState = { status: "idle", client: null, identity: null, info: null, error: null };

    constructor(options: NativeWatcherOptions = {}) {
        this.options = options;
        this.bridge = options.bridge ?? desktop;
    }

    get current(): DeviceState {
        return this.state;
    }

    subscribe(listener: (state: DeviceState) => void): () => void {
        this.listeners.add(listener);
        listener(this.state);
        return () => this.listeners.delete(listener);
    }

    /**
     * Start watching for hot-plug and adopt a device that is already attached.
     *
     * No gesture and no prompt, ever — that restriction is WebUSB's, and it is the whole reason the
     * hosted tier has to draw a Connect button it can never retire. Returns whether a device
     * connected; `false` just means nothing is plugged in.
     */
    async start(): Promise<boolean> {
        const channel = new Channel<UsbEvent>();
        channel.onmessage = (event) => this.onEvent(event);
        let devices: UsbDeviceSummary[];
        try {
            devices = await this.bridge.usbWatch(channel);
        } catch (cause) {
            this.publish({ ...this.state, status: "error", error: describe(cause) });
            return false;
        }
        return devices.length > 0 ? this.connect(devices[0]) : false;
    }

    /**
     * Look for a device now.
     *
     * The native counterpart of the browser's chooser, and deliberately not a dialog: this host can
     * see the device without asking anyone. The method stays because the UI's Connect button calls
     * it, and a button that re-scans is a reasonable thing for it to do on a host where auto-detect
     * already works.
     */
    async requestDevice(): Promise<boolean> {
        if (this.state.status === "ready") return true;
        this.publish({ ...this.state, status: "connecting", error: null });
        let devices: UsbDeviceSummary[];
        try {
            devices = await this.bridge.usbList();
        } catch (cause) {
            this.publish({ status: "error", client: null, identity: null, info: null, error: describe(cause) });
            return false;
        }
        if (devices.length === 0) {
            this.publish({
                status: "idle",
                client: null,
                identity: null,
                info: null,
                error: null,
            });
            return false;
        }
        return this.connect(devices[0]);
    }

    /**
     * A file on this disk, as an {@link ObjectSource} — the seam E3 (#913) reaches for when the map
     * to send is one this app just built.
     *
     * Reads `this.link` at call time rather than closing over a handle: a device that was unplugged
     * and put back is a *different* handle, and a source built against the old one would be sent to
     * an endpoint that no longer exists. Rejecting when there is no link is the same rule the pipes
     * hold — an operation on a device that is not there fails, it does not queue.
     */
    localFileSource = async (path: string): Promise<ObjectSource> => {
        const link = this.link;
        if (!link) throw new PipeError("closed", "No device is connected.");
        return nativeFileSource(link.info.handle, path);
    };

    /** Drop the link but keep watching, so re-plugging reconnects. */
    async disconnect(): Promise<void> {
        const client = this.state.client;
        this.publish({ status: "idle", client: null, identity: null, info: null, error: null });
        this.link = null;
        await client?.close();
    }

    /** Stop following this session. The backend's watch outlives it and is re-pointed on the next
     *  `start()`, which is what makes a window reload cheap. */
    async close(): Promise<void> {
        await this.disconnect();
        this.listeners.clear();
    }

    private async connect(device: UsbDeviceSummary): Promise<boolean> {
        this.publish({ ...this.state, status: "connecting", error: null });
        let link: NativeLink | null = null;
        try {
            link = await openNativeLink(device.id);
            const client = new ProtocolClient(link, { timeoutMs: this.options.timeoutMs });
            // The identity read is first on every connection and gates everything else: a version
            // mismatch is surfaced and stopped on rather than best-effort decoded (§1).
            const identity = await client.identity();
            const info = await client.deviceInfo();
            this.link = link;
            this.publish({ status: "ready", client, identity, info, error: null });
            return true;
        } catch (cause) {
            // A device claimed but never handshaken still holds its interface, and an interface can
            // be claimed once — so releasing it is what lets a retry, or another app, get at the
            // device instead of finding it busy.
            await link?.close().catch(() => undefined);
            this.link = null;
            this.publish({ status: "error", client: null, identity: null, info: null, error: describe(cause) });
            return false;
        }
    }

    private onEvent(event: UsbEvent): void {
        switch (event.type) {
            case "connected":
                if (this.state.status === "ready") return;
                void this.connect(event.device);
                return;
            case "disconnected": {
                if (!this.link || this.link.info.deviceId !== event.id) return;
                // Fail the pipes *before* awaiting anything: every pending read and write settles
                // now, so an in-flight transfer's UI reports "unplugged" instead of spinning.
                this.link.disconnected();
                const client = this.state.client;
                this.link = null;
                this.publish({ status: "idle", client: null, identity: null, info: null, error: null });
                void client?.close();
                return;
            }
            case "watchFailed":
                // Only meaningful when there is nothing connected: a live link keeps working
                // without hot-plug, and replacing a working device with an error message because
                // the OS stopped announcing *other* devices would be a lie about what is wrong.
                if (this.state.status === "idle") {
                    this.publish({ ...this.state, status: "error", error: event.message });
                }
                return;
        }
    }

    private publish(state: DeviceState): void {
        this.state = state;
        for (const listener of this.listeners) listener(state);
    }
}

function describe(cause: unknown): string {
    if (cause instanceof Error) return cause.message;
    const message = (cause as { message?: unknown } | null)?.message;
    return typeof message === "string" ? message : String(cause);
}
