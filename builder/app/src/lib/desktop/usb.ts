/**
 * The native USB transport (D4, #909): `nusb` behind C3's {@link BytePipe} seam.
 *
 * Tauri's webview has no WebUSB — WKWebView, WebView2 and WebKitGTK all lack it — so this tier
 * drives USB from Rust. #894 turns that into the point rather than the cost: it is the only
 * *universal* USB path, and the answer for every Safari and Firefox user the hosted site cannot
 * reach.
 *
 * **Nothing about the protocol lives here.** `lib/usb/` is §3's codec, §5.2's record framing and the
 * client, written once and validated against `specs/vectors/flat-store-v4/`; this file supplies the
 * two byte pipes underneath them and a watcher with the same three methods `WebUsbWatcher` has.
 * `FlatStoreClient` and `DeviceLink` are used unchanged, and they cannot tell which transport they
 * got — which was C3's whole design claim, and this is the thing that either makes it true or
 * exposes it.
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
 * - **Reads are still not messages.** The stream pipe hands back whatever one transfer delivered,
 *   and `records.ts` accumulates to the length the record's own prefix announced. §5.2 makes that
 *   *more* true here than it was under the v1 envelope, not less: a record spans packets by design.
 * - **There is no EP0 read.** §5.2.1's `GET_DEVICE_INFO` is a vendor control transfer, and the Rust
 *   side exposes no control-transfer command — so the link below omits `DeviceLink.vendorIn` and
 *   {@link FlatStoreClient.deviceInfo} answers `unavailable`. The connect flow publishes
 *   `info: null` rather than a fabricated firmware revision, which "an update is available" would
 *   then compare against.
 *
 * ## Why a map is not sent from disk here
 *
 * There used to be a `nativeFileSource` whose bytes went disk → endpoint inside Rust, never
 * entering the webview. Protocol v4 removes the seam it hung on: §3.8 requires **every** stream
 * record to be framed by the protocol client — `RequestId`, absolute offset, length, and §5.2's
 * record prefix around all of it — so `ObjectSource` has no `sendTo` any more, and the Rust
 * `usb_send_file` command writes raw file bytes with no framing at all. A native path built on it
 * would produce records the device reads as garbage, which is a worse outcome than a slower send.
 *
 * So the desktop app sends a map exactly the way the browser does: a picked `File` through the
 * webview, `blobSource`, `FlatStoreClient.put`. The alternative — teaching the Rust command to
 * frame records — is a real option and a Rust change, not a change here; it buys back the webview
 * hop, which the device's own card write dominates anyway (8.2 MB/s over sEMMC, #1158).
 */

import { PipeError, withAbort, type BytePipe, type DeviceLink } from "../usb/pipe";
import { DeviceError, FlatStoreClient, isFormatRecoveryState, type ClientOptions } from "../usb/client";
import type { DeviceInfo } from "../usb/records";
import type { DeviceState, DeviceWatcher } from "../usb/session";
import { Channel } from "@tauri-apps/api/core";
import { desktop, type UsbDeviceSummary, type UsbEvent, type UsbLinkInfo, type UsbPlane } from "./invoke";

// --- the pipe -----------------------------------------------------------------

/** One endpoint pair, as a byte pipe over the Tauri commands in `apps/obc-desktop/src/usb/`. */
class NativePipe implements BytePipe {
    readonly transport = "native";

    private failure: PipeError | null = null;
    private closedByUs = false;
    /** Rejectors for {@link dead}, released when the pipe closes or fails. */
    private readonly mourners: Array<(error: PipeError) => void> = [];
    /**
     * The previous {@link write}'s settlement — each write chains on it, so **submission order is
     * wire order**. Without the chain, `pumpChunks`'s `UPLOAD_WINDOW` concurrent writes each travel
     * as an independent IPC invoke, and the backend gives concurrent invokes no ordering: each
     * lands in its own task racing for the endpoint lock, so two in-flight chunks can swap on the
     * wire — right total length, wrong whole-object CRC, and the bigger the object the more likely
     * (the on-glass shard-CRC rejections of 2026-08-07). The device's per-packet turnaround is the
     * throughput ceiling by an order of magnitude, so what the chain costs is one IPC round-trip of
     * idle bridge per chunk, not wire idle. WebUSB needs none of this: the browser queues
     * per-endpoint transfers in call order.
     */
    private writeTail: Promise<void> = Promise.resolve();

    constructor(
        /**
         * The backend's name for this endpoint pair.
         *
         * `"bulk"` is what `DeviceLink.stream` is called on the Rust side — the pair's old name,
         * from when the channels were named after the endpoint type rather than after what §5 puts
         * on them. Renaming it is a Rust change (the `UsbPlane` enum, the command headers and
         * `apps/obc-desktop/src/usb/`), so the wire word stays `"bulk"` and the seam is here.
         */
        readonly plane: UsbPlane,
        private readonly handle: number,
        /**
         * The endpoint's max packet size, as `usb_open` reported it. Diagnostics only, never a
         * branch target: §5.2 gives packet boundaries no protocol meaning, and the backend chooses
         * its own transfer length for a read.
         */
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
                withAbort(desktop.usbRead(this.handle, this.plane), signal, "the read"),
                this.dead(),
            ]);
            // Never empty: the backend absorbs zero-length packets, which are USB-level markers
            // rather than data. A caller counting bytes towards a record's announced length could
            // not tell an empty read from a spurious wakeup, which is `BytePipe.read`'s contract.
            return new Uint8Array(body);
        } catch (cause) {
            throw this.asPipeError(cause, "read");
        } finally {
            release();
        }
    }

    /**
     * One OUT transfer of whatever the caller handed over.
     *
     * There is deliberately **no** "a control frame must fit one packet" rule here any more. Under
     * the v1 envelope a frame was one transfer, so a frame at exactly the packet size was
     * indistinguishable from one that had not ended and the host refused to send it. §5.2 replaces
     * that with a length prefix: records span packets by design, and the only thing that says where
     * a record ends is its own first two bytes. The old check would refuse the ordinary 4,112-byte
     * stream record this protocol is built around.
     */
    async write(bytes: Uint8Array, signal?: AbortSignal): Promise<void> {
        this.check();
        const run = async () => {
            const release = this.cancelOnAbort(signal, "out");
            try {
                await Promise.race([
                    withAbort(desktop.usbWrite(this.handle, this.plane, bytes), signal, "the write"),
                    this.dead(),
                ]);
            } catch (cause) {
                throw this.asPipeError(cause, "write");
            } finally {
                release();
            }
        };
        // Chain on the predecessor whether it settled well or badly — its caller owns its error;
        // this write only needs it to be off the bridge. See `writeTail` for why order is load-bearing.
        const chained = this.writeTail.then(run, run);
        this.writeTail = chained.then(
            () => {},
            () => {},
        );
        return chained;
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
            await desktop.usbReset(this.handle, this.plane);
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
        const onAbort = () => void desktop.usbCancel(this.handle, this.plane, dir).catch(() => undefined);
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

/**
 * Open and claim a device by the backend's opaque id, returning its two channels.
 *
 * `DeviceLink.vendorIn` is deliberately absent: §5.2.1's `GET_DEVICE_INFO` is an EP0 vendor
 * request, the Rust side has no control-transfer command, and omitting the method is what lets
 * {@link FlatStoreClient.deviceInfo} say "this host cannot ask" instead of answering with a version
 * nobody read off the device.
 */
export async function openNativeLink(deviceId: string): Promise<NativeLink> {
    let info: UsbLinkInfo;
    try {
        info = await desktop.usbOpen(deviceId);
    } catch (cause) {
        throw asPipeError(cause, "connection");
    }
    const control = new NativePipe("control", info.handle, info.controlPacketSize);
    // §5's stream channel, under the backend's own name for the pair — see {@link NativePipe.plane}.
    const stream = new NativePipe("bulk", info.handle, info.bulkPacketSize);
    return {
        info,
        control,
        stream,
        disconnected() {
            const error = new PipeError("closed", "The device was unplugged.");
            control.fail(error);
            stream.fail(error);
        },
        async close() {
            await control.close();
            await stream.close();
            try {
                await desktop.usbClose(info.handle);
            } catch {
                // Closing a link whose device is already gone is the normal unplug path.
            }
        },
    };
}

// --- discovery ----------------------------------------------------------------

export interface NativeWatcherOptions extends ClientOptions {
    /** Injected by tests; defaults to the real Tauri bridge. */
    bridge?: Bridge;
    /** Pause between hot-plug connect attempts; tests shrink it. */
    hotplugRetryDelayMs?: number;
}

/**
 * How a hot-plug connect is allowed to fail before it is a real error.
 *
 * The OS announces a device the moment it enumerates, which can be *before* it is claimable:
 * nusb's own hot-plug docs say to retry opening/claiming after a short delay, and macOS has the
 * same window while IOKit is still matching drivers against the fresh device. One failed attempt
 * at that instant must not park the session in a dead `error` state — that is exactly the
 * "plugged it in and nothing happened" bug, because the error chip reads as "No device".
 */
const HOTPLUG_CONNECT_ATTEMPTS = 5;
const HOTPLUG_RETRY_DELAY_MS = 400;

const delay = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

/** The slice of the Tauri bridge discovery uses, named so a test can drive the real code paths. */
export interface Bridge {
    usbWatch(onEvent: Channel<UsbEvent>): Promise<UsbDeviceSummary[]>;
    usbList(): Promise<UsbDeviceSummary[]>;
}

/**
 * Finds the device, keeps up with plugging and unplugging, and owns the {@link FlatStoreClient}.
 *
 * The same three-method shape as `WebUsbWatcher`, so `WatchedDeviceSession` wraps either one and
 * the UI above never learns which. Subscribers get an immutable snapshot per change.
 */
export class NativeWatcher implements DeviceWatcher {
    private readonly options: NativeWatcherOptions;
    private readonly bridge: Bridge;
    private readonly listeners = new Set<(state: DeviceState) => void>();

    private link: NativeLink | null = null;
    private state: DeviceState = { status: "idle", client: null, store: null, info: null, error: null };

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
     * connected; `false` just means nothing is plugged in. The initial adopt runs through the same
     * retrying flow the hot-plug path uses — an app launched moments after the cable went in hits
     * the same not-yet-claimable window a `Connected` event does.
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
        return devices.length > 0 ? this.adopt(devices[0]) : false;
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
        // Claim the flow before anything async: from here the click owns the published state, and
        // an adopt loop sleeping between attempts stands down instead of racing this flow for the
        // one interface claim.
        const token = this.claimFlow();
        this.publish({ ...this.state, status: "connecting", error: null });
        let devices: UsbDeviceSummary[];
        try {
            devices = await this.bridge.usbList();
        } catch (cause) {
            if (this.owns(token)) {
                this.publish({ status: "error", client: null, store: null, info: null, error: describe(cause) });
            }
            return false;
        }
        if (!this.owns(token)) return false;
        if (devices.length === 0) {
            this.publish({
                status: "idle",
                client: null,
                store: null,
                info: null,
                error: null,
            });
            return false;
        }
        return this.adopt(devices[0], token);
    }

    /** Drop the link but keep watching, so re-plugging reconnects. */
    async disconnect(): Promise<void> {
        // A deliberate disconnect outranks any connect flow still in flight: claim the flow so a
        // sleeping adopt loop stands down and an in-flight attempt finishes into silence.
        this.claimFlow();
        this.chasing = null;
        const client = this.state.client;
        this.publish({ status: "idle", client: null, store: null, info: null, error: null });
        this.link = null;
        await client?.close();
    }

    /** Stop following this session. The backend's watch outlives it and is re-pointed on the next
     *  `start()`, which is what makes a window reload cheap. */
    async close(): Promise<void> {
        await this.disconnect();
        this.listeners.clear();
    }

    // --- flow ownership ---------------------------------------------------------
    //
    // At most one connect flow may drive the published state and `this.link` at a time, but three
    // things can start one — the hot-plug adopt loop, the Connect click, `start()`'s initial probe
    // — and a running flow parks on awaits where any of the others (or a disconnect) can overtake
    // it. So every flow claims a monotonically increasing token, claiming invalidates every older
    // flow, and each older flow notices at its next step and stands down *silently*: it must not
    // publish over the winner, must not null a link it no longer owns, and — the expensive one —
    // must close any link it did open, because the Rust side holds the interface claim until it is
    // closed and an orphaned claim makes every later open fail "busy" until a physical replug.

    /** The current flow's token. Bump-to-claim; see the section comment. */
    private flow = 0;
    /** The device id the current flow is chasing while it has no link yet — how a `disconnected`
     *  event can end a flow whose device vanished mid-attempt. */
    private chasing: string | null = null;

    private claimFlow(): number {
        return ++this.flow;
    }

    private owns(token: number): boolean {
        return this.flow === token;
    }

    /**
     * One connect attempt: publish `connecting`, open, handshake, publish `ready` — with every
     * publish and every `this.link` write gated on still owning `token`. A failure publishes
     * nothing terminal (the state simply stays `connecting`) and returns the failure's sentence;
     * the *caller* owns the verdict, because only it knows whether more attempts are coming.
     */
    private async connectOnce(device: UsbDeviceSummary, token: number): Promise<{ ok: boolean; error?: string }> {
        if (!this.owns(token)) return { ok: false };
        this.publish({ ...this.state, status: "connecting", error: null });
        let link: NativeLink | null = null;
        try {
            link = await openNativeLink(device.id);
            const client = new FlatStoreClient(link, { timeoutMs: this.options.timeoutMs });
            // §5.2.1's EP0 read first, mirroring `WebUsbWatcher.connect` — except that this host
            // cannot issue one. `deviceInfo()` says so with `unavailable`, and **only** that code is
            // caught: a device that answered a request badly is a real failure and must not be
            // rounded down to "no info". Publishing `info: null` is the honest state until the Rust
            // bridge grows a control-transfer command; the alternative, a version this side made up,
            // is what "an update is available" would then compare against.
            let info: DeviceInfo | null;
            try {
                info = await client.deviceInfo();
            } catch (cause) {
                if (!(cause instanceof DeviceError) || cause.code !== "unavailable") throw cause;
                info = null;
            }
            // Then `LIST`, which §3 says every client issues before anything else and which is where
            // the store's identity and cache freshness come from. The wire major is not read at all:
            // §5.2 settles it by descriptor matching, and this host claimed the interface already.
            let store: DeviceState["store"];
            try {
                const page = await client.listPage({});
                store = { storeId: page.storeId, commitSequence: page.commitSequence };
            } catch (cause) {
                if (!isFormatRecoveryState(cause)) throw cause;
                store = null;
            }
            if (!this.owns(token)) {
                // A newer flow (or a disconnect) took the state while this handshake ran. The
                // connection itself is real, so it must be closed, not dropped — see the section
                // comment for what an orphaned interface claim costs.
                await client.close().catch(() => undefined);
                return { ok: false };
            }
            this.link = link;
            this.publish({
                status: "ready",
                client,
                store,
                info,
                error: null,
            });
            return { ok: true };
        } catch (cause) {
            // A device claimed but never handshaken still holds its interface, and an interface can
            // be claimed once — so releasing it is what lets a retry, or another app, get at the
            // device instead of finding it busy. `this.link` is deliberately not touched: this
            // flow never set it, and a newer flow's may already be in there.
            await link?.close().catch(() => undefined);
            return { ok: false, error: describe(cause) };
        }
    }

    /**
     * Connect to a device, retrying over the window in which the OS has announced it but not
     * finished setting it up (see {@link HOTPLUG_CONNECT_ATTEMPTS}) — the shared flow behind the
     * hot-plug event, `start()`'s initial probe and the Connect click. Between attempts, and again
     * before giving up, the loop re-checks that the device is still attached: a re-unplug ends in
     * `idle`, not in an error about a device that is not there.
     */
    private async adopt(device: UsbDeviceSummary, token: number = this.claimFlow()): Promise<boolean> {
        if (this.owns(token)) this.chasing = device.id;
        try {
            let lastError = "The device could not be opened.";
            for (let attempt = 1; attempt <= HOTPLUG_CONNECT_ATTEMPTS; attempt++) {
                const result = await this.connectOnce(device, token);
                if (result.ok) return true;
                // Superseded (a newer event, a Connect click, a disconnect) — stand down silently
                // rather than fighting the newer flow over the published state. This covers the
                // final attempt too: its failure must not bury a successor's `connecting`.
                if (!this.owns(token)) return false;
                if (result.error !== undefined) lastError = result.error;
                if (attempt < HOTPLUG_CONNECT_ATTEMPTS) {
                    await delay(this.options.hotplugRetryDelayMs ?? HOTPLUG_RETRY_DELAY_MS);
                    if (!this.owns(token)) return false;
                }
                // Still attached? Between attempts this decides whether to keep trying; after the
                // final one it decides between the honest `idle` and the device's own error. A
                // probe that itself fails proves nothing, so the flow keeps its own course.
                const present = await this.stillAttached(device.id);
                if (!this.owns(token)) return false;
                if (present === false) {
                    this.publish({ status: "idle", client: null, store: null, info: null, error: null });
                    return false;
                }
            }
            this.publish({ status: "error", client: null, store: null, info: null, error: lastError });
            return false;
        } finally {
            if (this.owns(token)) this.chasing = null;
        }
    }

    /** Whether `deviceId` is still listed; null when the probe itself failed (unknown). */
    private async stillAttached(deviceId: string): Promise<boolean | null> {
        try {
            return (await this.bridge.usbList()).some((d) => d.id === deviceId);
        } catch {
            return null;
        }
    }

    private onEvent(event: UsbEvent): void {
        switch (event.type) {
            case "connected":
                if (this.state.status === "ready") return;
                void this.adopt(event.device);
                return;
            case "disconnected": {
                if (this.link && this.link.info.deviceId === event.id) {
                    // Fail the pipes *before* awaiting anything: every pending read and write
                    // settles now, so an in-flight transfer's UI reports "unplugged" instead of
                    // spinning.
                    this.link.disconnected();
                    const client = this.state.client;
                    this.link = null;
                    this.publish({ status: "idle", client: null, store: null, info: null, error: null });
                    void client?.close();
                    return;
                }
                // No link yet, but a connect flow is chasing exactly this device: end it in the
                // honest state now. The in-flight attempt fails on its own, sees a stale token,
                // and says nothing.
                if (this.chasing === event.id) {
                    this.claimFlow();
                    this.chasing = null;
                    this.publish({ status: "idle", client: null, store: null, info: null, error: null });
                }
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
