/**
 * The WebUSB transport: `navigator.usb` behind the {@link BytePipe} seam.
 *
 * ## The permission rule the UI has to be built around
 *
 * `requestDevice()` — the browser's device chooser — may only be called from a **user gesture**.
 * That is a hard browser rule, not a preference, and it shapes the product rather than being an
 * implementation detail:
 *
 * - **First visit**: nothing can be detected. The page has to show a real button ("Connect your
 *   OBC") and the chooser opens from the click. There is no way to probe first and prompt only if
 *   something is there.
 * - **Every visit after**: `getDevices()` returns the devices the user already granted, with no
 *   gesture and no prompt. Combined with the `connect` / `disconnect` events, that is the wanted
 *   UX — plug in the device and the page lights up on its own — and it is why {@link WebUsbWatcher}
 *   starts by adopting rather than by asking.
 * - **Permission is per-origin and per-device**, and it survives reloads. It does *not* survive the
 *   user clearing site data, so the connect button can never be retired from the UI.
 *
 * C4 (#903) and C5 (#904) inherit that shape: an idle state with a button, an auto-detected state
 * that needs no interaction, and no flow that assumes a device can be found on page load.
 *
 * ## Browser reach
 *
 * WebUSB is Chromium-only and needs a secure context. Firefox and Safari get
 * {@link PipeError} `unsupported`, and the honest answer for them is the desktop app (#894) — the
 * fallback is not a degraded USB path, it is the existing download-and-copy flow.
 *
 * ## The endpoint layout, and where the version comes from
 *
 * The layout below — a vendor-specific interface, lowest IN/OUT pair control, next pair stream — is
 * what the firmware descriptors declare, so {@link discoverLayout}'s rule is a contract rather than
 * a guess. All four endpoints are 512 bytes: the LM20's USBHS is a high-speed core, and high-speed
 * bulk endpoints are 512 bytes by USB rule. Packet size is **not** a protocol number any more —
 * `FLAT_Store_Protocol.md` §5.2 lets a record span packets, and `records.ts` is what reassembles
 * them.
 *
 * **The wire major is settled by matching, before a record moves** (§5.2). The vendor interface
 * reports `bInterfaceProtocol = 4` and the device descriptor's `bcdDevice` carries the major in its
 * high byte, `0x0400`. {@link checkWireMajor} reads both and refuses a device that says anything
 * else — there is no version *read* on this link, and adding one back would be the duplication the
 * major bump removed.
 *
 * The **VID/PID is still provisional on purpose** — see {@link OBC_USB_FILTERS}. Allocating a real
 * product id is an owner action, not a code change; when it happens, this constant and the
 * firmware's `PRODUCT_ID` move together.
 */

import { PipeError, withAbort, type BytePipe, type DeviceLink } from "./pipe";
import { FlatStoreClient, type ClientOptions } from "./client";
import type { DeviceInfo } from "./records";
import { WIRE_MAJOR } from "./protocol";

// --- the slice of WebUSB this file uses ---------------------------------------
//
// Declared structurally rather than pulled from `@types/w3c-web-usb`, for two reasons: it documents
// exactly which of the API's surface the transport depends on, and it makes the whole thing
// injectable, so `webusb.test.ts` drives the real code paths under Node with a scripted device.

/** What a `transferIn` settles with. Named because the pipe holds one across a cancelled read. */
export interface UsbInResult {
    data?: DataView;
    status: string;
}

/** What a `transferOut` settles with. */
export interface UsbOutResult {
    bytesWritten: number;
    status: string;
}

export interface UsbEndpointLike {
    endpointNumber: number;
    direction: "in" | "out";
    type: string;
    packetSize: number;
}

export interface UsbInterfaceLike {
    interfaceNumber: number;
    alternate: {
        interfaceClass: number;
        /** §5.2's `bInterfaceProtocol` — the wire major, readable before a record is exchanged. */
        interfaceProtocol?: number;
        endpoints: UsbEndpointLike[];
    };
}

/** What a `controlTransferIn` settles with. */
export interface UsbControlInResult {
    data?: DataView;
    status: string;
}

/** The setup packet §5.2.1 needs: device-to-host, vendor, recipient interface. */
export interface UsbControlSetup {
    requestType: "vendor";
    recipient: "interface";
    request: number;
    value: number;
    index: number;
}

export interface UsbConfigurationLike {
    configurationValue: number;
    interfaces: UsbInterfaceLike[];
}

export interface UsbDeviceLike {
    readonly vendorId: number;
    readonly productId: number;
    /** `bcdDevice`'s high byte — §5.2's other statement of the wire major. */
    readonly deviceVersionMajor?: number;
    readonly serialNumber?: string;
    readonly productName?: string;
    readonly opened: boolean;
    readonly configuration: UsbConfigurationLike | null;
    open(): Promise<void>;
    close(): Promise<void>;
    selectConfiguration(value: number): Promise<void>;
    claimInterface(interfaceNumber: number): Promise<void>;
    releaseInterface(interfaceNumber: number): Promise<void>;
    transferIn(endpointNumber: number, length: number): Promise<UsbInResult>;
    transferOut(endpointNumber: number, data: Uint8Array): Promise<UsbOutResult>;
    controlTransferIn(setup: UsbControlSetup, length: number): Promise<UsbControlInResult>;
    clearHalt(direction: "in" | "out", endpointNumber: number): Promise<void>;
}

export interface UsbConnectionEventLike {
    device: UsbDeviceLike;
}

/** The `navigator.usb` surface. */
export interface UsbLike {
    getDevices(): Promise<UsbDeviceLike[]>;
    requestDevice(options: { filters: Array<{ vendorId?: number; productId?: number }> }): Promise<UsbDeviceLike>;
    addEventListener(type: "connect" | "disconnect", listener: (event: UsbConnectionEventLike) => void): void;
    removeEventListener(type: "connect" | "disconnect", listener: (event: UsbConnectionEventLike) => void): void;
}

/**
 * The device filter.
 *
 * `1209:0001` is pid.codes' **prototype / testing** pair, chosen deliberately: it is the id that
 * says "not allocated yet". The firmware declares the same pair, so the two agree today; the real
 * allocation is an owner action (a PID request against the pid.codes repository), and when it lands
 * this constant and the firmware's `PRODUCT_ID` change together.
 */
export const OBC_USB_FILTERS: ReadonlyArray<{ vendorId: number; productId?: number }> = [
    { vendorId: 0x1209, productId: 0x0001 },
];

/** USB vendor-specific interface class — the class a WebUSB-reachable interface must use. */
const VENDOR_CLASS = 0xff;

// --- the pipe -----------------------------------------------------------------

/**
 * An IN transfer whose result nobody has taken yet.
 *
 * `settled` tracks the *wire*, not the caller: a transfer that has completed is off the endpoint
 * even while its bytes are still waiting for a reader, and those are two different questions —
 * {@link WebUsbPipe.reset} asks the first, {@link WebUsbPipe.read} the second.
 */
interface HeldTransfer {
    readonly transfer: Promise<UsbInResult>;
    settled: boolean;
}

/** One endpoint pair as a byte pipe. */
class WebUsbPipe implements BytePipe {
    readonly transport = "webusb";

    private failure: PipeError | null = null;
    private closedByUs = false;
    /** Rejectors for {@link dead}, released when the pipe closes or fails. */
    private readonly mourners: Array<(error: PipeError) => void> = [];
    /** The IN transfer no reader has taken the result of — a cancelled read's, kept rather than
     *  abandoned. At most one, because {@link receive} claims before it submits.
     *
     *  That claim-then-submit also assumes **one reader at a time** per pipe, which is what the
     *  layers above provide: the control channel has a single read loop, and the client's
     *  one-transfer gate (§1) serialises every stream read. Two concurrent readers would share this
     *  one transfer and be handed the same bytes twice rather than a packet each. */
    private heldIn: HeldTransfer | null = null;
    /** OUT transfers still on the wire.
     *
     *  Counted rather than flagged, and that stopped being a precaution: an upload keeps up to
     *  `UPLOAD_WINDOW` (`client.ts`) transfers queued at the endpoint, so this is routinely > 1 and
     *  a flag would be cleared by the first of them to settle. Only {@link reset} reads it, to
     *  decide whether the OUT half is idle enough to `clearHalt` — which stays correct at any depth,
     *  because "anything still on the wire" is exactly the question. Ordering between concurrent
     *  writes is WebUSB's: transfers submitted to one endpoint are delivered in submission order,
     *  which is what makes a windowed upload a byte stream rather than a race. */
    private writesInFlight = 0;

    constructor(
        readonly kind: "control" | "stream",
        private readonly device: UsbDeviceLike,
        private readonly inEndpoint: number,
        private readonly outEndpoint: number,
        private readonly packetSize: number,
    ) {}

    get open(): boolean {
        return !this.failure && !this.closedByUs;
    }

    /**
     * One bulk IN transfer.
     *
     * The request is exactly one max packet, which is the only length that cannot stall: a USB IN
     * transfer completes when the requested length is reached **or** a short packet arrives, so
     * asking for eight packets from a device that sends four and then pauses waits forever. The
     * cost is a transfer per packet in the download direction — acceptable because the objects that
     * travel that way (rides, catalogs, diagnostics) are tens of kilobytes. The upload direction,
     * where the multi-megabyte maps go, is not affected: `transferOut` takes whatever it is given.
     */
    async read(signal?: AbortSignal): Promise<Uint8Array> {
        this.check();
        for (;;) {
            const result = await this.receive(signal);
            // `stall` and `babble` both mean the endpoint is no longer trustworthy — anything but
            // `ok` has to surface, not be read past.
            if (result.status !== "ok") {
                throw new PipeError(
                    "device-error",
                    `The device answered "${result.status}" on endpoint ${this.inEndpoint}.`,
                );
            }
            const bytes = result.data ? new Uint8Array(result.data.buffer, result.data.byteOffset, result.data.byteLength) : null;
            // A zero-length packet is a USB-level marker, not data; a caller that got an empty array
            // could not tell it from a spurious wakeup, so absorb it and wait for real bytes.
            if (bytes && bytes.length > 0) return bytes.slice();
        }
    }

    /**
     * One OUT transfer of whatever the caller handed over.
     *
     * There is deliberately **no** "must fit one packet" rule here any more. Under the v1 envelope a
     * control frame was one transfer and a frame at exactly the packet size was indistinguishable
     * from one that had not ended, so the host refused to send it. §5.2 replaces that with a length
     * prefix: packet boundaries carry no protocol meaning, records span packets by design, and the
     * only thing that says where a record ends is its own first two bytes. Keeping the old check
     * would refuse the ordinary 4,112-byte stream record this protocol is built around.
     */
    async write(bytes: Uint8Array, signal?: AbortSignal): Promise<void> {
        this.check();
        const result = await this.transfer(() => this.send(bytes), signal, "the write");
        if (result.status !== "ok") {
            throw new PipeError("device-error", `The device answered "${result.status}" to a write.`);
        }
    }

    /**
     * Return the endpoint pair to a known-empty state — by argument, because WebUSB will not let it
     * be done by force.
     *
     * A transfer that ends before its last record leaves the channel at an unknown offset, so it is
     * emptied before another transfer uses it. BLE closes and reopens the CoC; D4's native pipe
     * cancels every URB, drains the completions, then clears the halt
     * (`apps/obc-desktop/src/usb/link.rs`). This pipe can do only the last of those three.
     * `clearHalt` is a `CLEAR_FEATURE(ENDPOINT_HALT)` control request — it neither cancels a
     * transfer nor discards a byte — and the WebUSB API has no per-transfer cancel at all; the only
     * thing that aborts a submitted transfer is `close()`, which would take the control plane's read
     * loop down with it and turn every cancelled download into a reconnect. So the emptiness rests
     * on three facts, none of which is `clearHalt`:
     *
     * - **Nothing is buffered on the IN side.** A bulk IN endpoint delivers only into an outstanding
     *   transfer; with none submitted there is no host-side buffer at all, and the unread bytes are
     *   still in the device, where §3.8's cancel has it drop the transfer.
     * - **Stray OUT bytes are the device's problem, and it handles them.** An aborted write leaves a
     *   `transferOut` that will still be delivered. §3.8 has the device discard stream frames bearing
     *   a `RequestId` that is not the live transfer's — in silence, because late frames from a
     *   transfer the peer has been told about are ordinary in-flight traffic — which is exactly what
     *   makes that orphan harmless, and why the client never reuses a `RequestId`.
     * - **A cancelled read's transfer is kept, not orphaned — if it is still pending.** This is the
     *   one C3 got wrong, and the cancel handshake is why it matters rather than why it doesn't.
     *   `FlatStoreClient` waits for the cancelled transfer's own `cancelled` response, and a device
     *   that has answered it sends **nothing more** for that object — so a transfer still pending
     *   then will
     *   never see a stale byte, and will never complete on its own either. It just sits on the
     *   endpoint. Submitting a fresh `transferIn` for the next object would queue *behind* it, and
     *   the abandoned one would take that object's first packet and drop it on the floor: a
     *   download one packet short, parked forever on a read the device had already satisfied, and
     *   every later cancel adding another. {@link receive} keeps it for the next reader instead,
     *   which is both correct and free — every read requests the same one max packet, so a transfer
     *   submitted for one object fits the next. Note it keeps the *result*, not merely the
     *   transfer: the device commonly answers while no read is outstanding (that is the whole
     *   reason the abort raced one), and letting that value fall on the floor loses the packet just
     *   as thoroughly.
     * - **A cancelled read's transfer is dropped if it has already settled.** "Pending" above is
     *   load-bearing, and the mirror image is just as wrong: a device that is mid-stream keeps
     *   pushing until the abort reaches it, so the transfer the caller walked away from may have
     *   taken one last packet of the **aborted** object. Keeping that would prepend a stale packet
     *   to the next one. This is where reset earns its name: it runs inside `withTransferSlot`'s
     *   failure path, *before* the slot is released and so before any next descriptor exists, which
     *   makes the test total — a held result that has settled by now took bytes of the exchange
     *   being abandoned, and there is nothing else it could be. Drop it, and the same line disposes
     *   of a held `stall` or transfer error, which carries no bytes and describes an endpoint this
     *   reset is about to clear anyway.
     *
     * Which leaves `clearHalt` doing its actual job, un-sticking a stalled endpoint, and only where
     * that can be true. A half with a transfer **still on the wire** is skipped: it cannot be
     * halted (a stall would have completed the transfer), so there is no halt to clear, and clearing
     * one would reset the endpoint's data toggle underneath a live transfer — the same rule D4
     * meets by cancelling and draining first. The transfer discarded just above does not block it:
     * having completed, it is off the endpoint, and a stall is exactly what that endpoint may now
     * be idle *from*.
     * Best-effort otherwise — an endpoint that was never halted may reject the request, and that is
     * not a failure of the reset.
     */
    async reset(): Promise<void> {
        this.check();
        // Settled by now means it took bytes of the exchange being abandoned — the next descriptor
        // has not been sent yet, so there is nothing else those bytes could belong to.
        if (this.heldIn?.settled) this.heldIn = null;
        for (const [direction, endpoint, onTheWire] of [
            ["in", this.inEndpoint, this.heldIn !== null && !this.heldIn.settled],
            ["out", this.outEndpoint, this.writesInFlight > 0],
        ] as const) {
            if (onTheWire) continue;
            try {
                await this.device.clearHalt(direction, endpoint);
            } catch {
                // Not halted, or the browser declined; either way there is nothing else to do here.
            }
        }
    }

    async close(): Promise<void> {
        this.closedByUs = true;
        this.bury(new PipeError("closed", "The device link is closed."));
    }

    /**
     * Mark the pipe dead and fail everything waiting on it.
     *
     * Called from the `disconnect` event rather than discovered by a transfer timing out: a pulled
     * cable leaves `transferIn` pending indefinitely in some browsers, and waiting for it is the
     * stuck spinner #902 asks to not have.
     */
    fail(error: PipeError): void {
        this.failure ??= error;
        this.bury(error);
    }

    /**
     * A promise that rejects when this pipe closes or fails.
     *
     * Every transfer races against it, because a submitted `transferIn` **cannot be cancelled**:
     * closing the device is supposed to reject one, but relying on that leaves the read loop parked
     * forever the one time a browser doesn't. Racing settles the caller either way and lets the
     * orphan resolve into nothing.
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

    /**
     * One IN transfer's result: the held one if a cancelled read left it, a fresh one otherwise.
     *
     * The result is held, not just the transfer — the device may well answer while nobody is
     * reading, and dropping the value then would lose the packet exactly as surely as dropping the
     * transfer would. So the hold is released only when a caller has actually been given the
     * outcome; a caller who walks away leaves it standing. {@link reset} has the full argument.
     */
    private async receive(signal?: AbortSignal): Promise<UsbInResult> {
        try {
            const result = await this.transfer(() => (this.heldIn ??= this.hold()).transfer, signal, "the read");
            this.heldIn = null;
            return result;
        } catch (cause) {
            // Only a cancelled *caller* leaves a transfer worth keeping. Anything else — a stall, a
            // dead pipe — is the transfer's own end, and it has now been reported to someone.
            if (!(cause instanceof PipeError) || cause.code !== "aborted") this.heldIn = null;
            throw cause;
        }
    }

    private hold(): HeldTransfer {
        const held: HeldTransfer = {
            transfer: this.device.transferIn(this.inEndpoint, this.packetSize),
            settled: false,
        };
        const done = () => {
            held.settled = true;
        };
        // Observing both outcomes also means a held transfer that fails unread is never an
        // unhandled rejection — the next reader still sees it, once.
        void held.transfer.then(done, done);
        return held;
    }

    /**
     * One OUT transfer, counted but never held — the next writer has its own bytes to send, and an
     * abandoned write's bytes are already the device's to discard ({@link reset}).
     */
    private send(bytes: Uint8Array): Promise<UsbOutResult> {
        const transfer = this.device.transferOut(this.outEndpoint, bytes);
        this.writesInFlight += 1;
        const done = () => {
            this.writesInFlight -= 1;
        };
        void transfer.then(done, done);
        return transfer;
    }

    /** Run one WebUSB transfer, translating its failures and honouring cancellation. */
    private async transfer<T>(run: () => Promise<T>, signal: AbortSignal | undefined, what: string): Promise<T> {
        let promise: Promise<T>;
        try {
            promise = run();
        } catch (cause) {
            throw this.asPipeError(cause);
        }
        try {
            // Cancelling releases the *caller*, never the transfer: WebUSB cannot cancel a submitted
            // one. What becomes of the transfer left on the endpoint is `reset`'s subject, and it is
            // not "nothing" — an abandoned read is held for the next reader rather than discarded.
            return await Promise.race([withAbort(promise, signal, what), this.dead()]);
        } catch (cause) {
            if (cause instanceof PipeError) throw cause;
            throw this.asPipeError(cause);
        }
    }

    private asPipeError(cause: unknown): PipeError {
        if (this.failure) return this.failure;
        const name = (cause as { name?: string } | null)?.name;
        // A vanished device surfaces as NetworkError / NotFoundError depending on when it went.
        if (name === "NetworkError" || name === "NotFoundError") {
            const error = new PipeError("closed", "The device was disconnected.", { cause });
            this.fail(error);
            return error;
        }
        return new PipeError("device-error", describe(cause), { cause });
    }
}

/** Where the two pipes live on the device's vendor interface. */
export interface EndpointLayout {
    interfaceNumber: number;
    control: { in: number; out: number; packetSize: number };
    stream: { in: number; out: number; packetSize: number };
}

/**
 * Pick the vendor interface and split its endpoints into a control pair and a stream pair.
 *
 * The rule — lowest-numbered IN/OUT pair is control, the next is stream — is what the firmware
 * descriptors declare, so this is a contract rather than a guess. It is deliberately mechanical so
 * the descriptor can be read off it, and {@link openWebUsbLink} takes an explicit layout for a
 * device whose real one differs.
 */
export function discoverLayout(configuration: UsbConfigurationLike): EndpointLayout {
    const iface = configuration.interfaces.find((i) => i.alternate.interfaceClass === VENDOR_CLASS);
    if (!iface) {
        throw new PipeError(
            "device-error",
            "This device has no vendor-specific interface, so the browser cannot talk to it.",
        );
    }
    const ins = iface.alternate.endpoints.filter((e) => e.direction === "in").sort(byNumber);
    const outs = iface.alternate.endpoints.filter((e) => e.direction === "out").sort(byNumber);
    if (ins.length < 2 || outs.length < 2) {
        throw new PipeError(
            "device-error",
            `The device's interface exposes ${ins.length} IN and ${outs.length} OUT endpoints; ` +
                "two of each are needed (a control pair and a stream pair).",
        );
    }
    return {
        interfaceNumber: iface.interfaceNumber,
        control: { in: ins[0].endpointNumber, out: outs[0].endpointNumber, packetSize: ins[0].packetSize },
        stream: { in: ins[1].endpointNumber, out: outs[1].endpointNumber, packetSize: ins[1].packetSize },
    };
}

/**
 * Refuse a device that does not announce wire major {@link WIRE_MAJOR} (§5.2).
 *
 * Both statements are checked, and neither is required to be present: WebUSB exposes
 * `deviceVersionMajor` everywhere but `interfaceProtocol` only on `alternate`, and a test harness
 * that scripts one of them should not have to script both. What is refused is a device that
 * *contradicts* the major — saying nothing is treated as an older descriptor and left to fail on the
 * first exchange, where the failure names an actual message rather than a missing field.
 *
 * The message shape is the one the v1 identity read used, because the rider's two options have not
 * changed: the device is behind, or the page is.
 */
export function checkWireMajor(device: UsbDeviceLike, layout: EndpointLayout, configuration: UsbConfigurationLike): void {
    const iface = configuration.interfaces.find((i) => i.interfaceNumber === layout.interfaceNumber);
    const stated = [device.deviceVersionMajor, iface?.alternate.interfaceProtocol].filter(
        (value): value is number => typeof value === "number",
    );
    const wrong = stated.find((value) => value !== WIRE_MAJOR);
    if (wrong !== undefined) {
        throw new PipeError(
            "device-error",
            `This device speaks protocol v${wrong}; this page speaks v${WIRE_MAJOR}. ` +
                "Update the device firmware, or reload the page for a newer build.",
        );
    }
}

function byNumber(a: UsbEndpointLike, b: UsbEndpointLike): number {
    return a.endpointNumber - b.endpointNumber;
}

/** A claimed device, as a {@link DeviceLink}. */
export interface WebUsbLink extends DeviceLink {
    readonly device: UsbDeviceLike;
    /** Fail both pipes at once — what the `disconnect` event calls. */
    disconnected(): void;
}

/**
 * Open, configure and claim a device, returning its two channels and its EP0 read.
 *
 * The wire major is checked here, between the claim and the first record: §5.2 settles it by
 * matching, and a device that says something else must never be handed to a client that would then
 * misparse every frame it sent.
 */
export async function openWebUsbLink(device: UsbDeviceLike, layout?: EndpointLayout): Promise<WebUsbLink> {
    if (!device.opened) await device.open();
    if (!device.configuration) await device.selectConfiguration(1);
    const configuration = device.configuration;
    if (!configuration) throw new PipeError("device-error", "The device offers no USB configuration.");
    const chosen = layout ?? discoverLayout(configuration);
    checkWireMajor(device, chosen, configuration);
    await device.claimInterface(chosen.interfaceNumber);

    const control = new WebUsbPipe("control", device, chosen.control.in, chosen.control.out, chosen.control.packetSize);
    const stream = new WebUsbPipe("stream", device, chosen.stream.in, chosen.stream.out, chosen.stream.packetSize);
    return {
        device,
        control,
        stream,
        /**
         * §5.2.1's EP0 vendor request. Recipient **interface** rather than device, so it cannot
         * collide with the device-level MS OS 2.0 descriptor request the same device answers for
         * Windows; `wIndex` is therefore the interface this link claimed.
         */
        async vendorIn(request: number, value: number, length: number, signal?: AbortSignal): Promise<Uint8Array> {
            // The seam offers a signal and this implementation used to drop it, which made every
            // caller's timeout a lie on this one call. WebUSB has no cancel for a control transfer,
            // so the honest shape is the pre-flight check plus a race: the transfer is not stopped,
            // but the caller stops waiting for it, which is the difference between a bounded failure
            // and a spinner. A late resolution is discarded by the `Promise.race` and by nothing
            // else needing it — `withAbort` is the same helper both byte pipes use, and its own docs
            // are careful about exactly this distinction.
            let result: UsbControlInResult;
            try {
                const transfer = device.controlTransferIn(
                    {
                        requestType: "vendor",
                        recipient: "interface",
                        request,
                        value,
                        index: chosen.interfaceNumber,
                    },
                    length,
                );
                result = await withAbort(transfer, signal, "the device-info read");
            } catch (cause) {
                if (cause instanceof PipeError) throw cause;
                throw new PipeError("device-error", describe(cause), { cause });
            }
            if (result.status !== "ok") {
                throw new PipeError("device-error", `The device answered "${result.status}" to a control request.`);
            }
            const data = result.data;
            // §5.2.1 says the device returns a short transfer, so an empty one is a device that
            // stalled the request in all but name rather than a device with no strings.
            if (!data || data.byteLength === 0) {
                throw new PipeError("device-error", "The device returned nothing for a control request.");
            }
            return new Uint8Array(data.buffer, data.byteOffset, data.byteLength).slice();
        },
        disconnected() {
            const error = new PipeError("closed", "The device was unplugged.");
            control.fail(error);
            stream.fail(error);
        },
        async close() {
            await control.close();
            await stream.close();
            try {
                await device.releaseInterface(chosen.interfaceNumber);
                await device.close();
            } catch {
                // Closing a device that is already gone is the normal unplug path.
            }
        },
    };
}

// --- discovery ----------------------------------------------------------------

/** What the UI renders. */
export type DeviceStatus = "unsupported" | "idle" | "connecting" | "ready" | "error";

/**
 * The store the connected device holds, as `LIST` reports it (§3.3).
 *
 * This is what the v1 identity read used to carry, minus everything the descriptors now answer. A
 * `StoreId` a client has not seen means the card was re-initialized and everything it cached is
 * void; the commit sequence is how it learns of a movement it did not cause.
 */
export interface StoreIdentity {
    readonly storeId: string;
    readonly commitSequence: bigint;
}

/** The watcher's whole observable state, handed to subscribers as one immutable snapshot. */
export interface DeviceState {
    status: DeviceStatus;
    /** Non-null exactly when `status === "ready"`. */
    client: FlatStoreClient | null;
    /** The card's identity, read back from the `LIST` every client issues first. */
    store: StoreIdentity | null;
    /** The three §5.2.1 strings, or `null` where this host cannot issue an EP0 request. */
    info: DeviceInfo | null;
    /** A message written for the rider, non-null exactly when `status === "error"`. */
    error: string | null;
}

export interface WatcherOptions extends ClientOptions {
    filters?: ReadonlyArray<{ vendorId: number; productId?: number }>;
    layout?: EndpointLayout;
    /** Injected for tests; defaults to `navigator.usb`. */
    usb?: UsbLike;
}

/**
 * Finds the device, keeps up with plugging and unplugging, and owns the {@link FlatStoreClient}.
 *
 * Framework-free on purpose — `session.svelte.ts` is a thin reactive shell over this, and D4 reuses
 * it unchanged behind a native transport. Subscribers get an immutable snapshot per change, so a
 * consumer can hold on to one without watching it mutate underneath.
 */
export class WebUsbWatcher {
    private readonly usb: UsbLike | null;
    private readonly options: WatcherOptions;
    private readonly listeners = new Set<(state: DeviceState) => void>();

    private link: WebUsbLink | null = null;
    private state: DeviceState;

    constructor(options: WatcherOptions = {}) {
        this.options = options;
        this.usb = options.usb ?? webUsb();
        this.state = {
            status: this.usb ? "idle" : "unsupported",
            client: null,
            store: null,
            info: null,
            error: this.usb
                ? null
                : "This browser cannot talk to USB devices. Chrome, Edge and other Chromium browsers can — " +
                  "or use the desktop app, which works everywhere.",
        };
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
     * Adopt an already-permitted device and start watching for hot-plug.
     *
     * **No gesture needed and no prompt shown**, which is what makes "plug it in and the page lights
     * up" possible. Returns whether a device connected; `false` is the ordinary first-visit answer,
     * not an error.
     */
    async start(): Promise<boolean> {
        if (!this.usb) return false;
        this.usb.addEventListener("connect", this.onConnect);
        this.usb.addEventListener("disconnect", this.onDisconnect);
        const devices = await this.usb.getDevices();
        const match = devices.find((d) => this.matches(d));
        if (!match) return false;
        return this.connect(match);
    }

    /**
     * Show the browser's device chooser.
     *
     * **Must be called synchronously from a user gesture** — a click handler, not a `setTimeout` or
     * a promise continuation after an `await`. A call outside one throws `SecurityError`, and no
     * amount of host-side cleverness works around it.
     */
    async requestDevice(): Promise<boolean> {
        if (!this.usb) return false;
        let device: UsbDeviceLike;
        try {
            device = await this.usb.requestDevice({ filters: [...(this.options.filters ?? OBC_USB_FILTERS)] });
        } catch (cause) {
            // Dismissing the chooser is a NotFoundError, and a cancelled prompt is not an error the
            // rider needs told about — they just closed a dialog.
            if ((cause as { name?: string } | null)?.name === "NotFoundError") return false;
            this.publish({ ...this.state, status: "error", error: describe(cause) });
            return false;
        }
        return this.connect(device);
    }

    /** Drop the link but keep watching, so re-plugging reconnects. */
    async disconnect(): Promise<void> {
        const client = this.state.client;
        this.publish({ status: this.usb ? "idle" : "unsupported", client: null, store: null, info: null, error: null });
        await client?.close();
        this.link = null;
    }

    /** Stop watching entirely. */
    async close(): Promise<void> {
        this.usb?.removeEventListener("connect", this.onConnect);
        this.usb?.removeEventListener("disconnect", this.onDisconnect);
        await this.disconnect();
        this.listeners.clear();
    }

    private async connect(device: UsbDeviceLike): Promise<boolean> {
        this.publish({ ...this.state, status: "connecting", error: null });
        let link: WebUsbLink | null = null;
        try {
            link = await openWebUsbLink(device, this.options.layout);
            const client = new FlatStoreClient(link, { timeoutMs: this.options.timeoutMs });
            // §5.2.1 first, because it sits below the record framing: the firmware revision is what
            // "an update is available" compares against, and it is readable the moment the interface
            // is claimed. Then `LIST`, which §3 says every client issues before it does anything
            // else and which is where the store's identity and cache freshness come from.
            const info = await client.deviceInfo();
            const page = await client.listPage({});
            this.link = link;
            this.publish({
                status: "ready",
                client,
                store: { storeId: page.storeId, commitSequence: page.commitSequence },
                info,
                error: null,
            });
            return true;
        } catch (cause) {
            // A device claimed but never handshaken still holds its interface. Releasing it is what
            // lets a retry — or another tab — get at the device instead of finding it busy.
            await link?.close().catch(() => undefined);
            this.link = null;
            this.publish({ status: "error", client: null, store: null, info: null, error: describe(cause) });
            return false;
        }
    }

    private matches(device: UsbDeviceLike): boolean {
        return (this.options.filters ?? OBC_USB_FILTERS).some(
            (f) => f.vendorId === device.vendorId && (f.productId === undefined || f.productId === device.productId),
        );
    }

    private readonly onConnect = (event: UsbConnectionEventLike): void => {
        if (this.state.status === "ready" || !this.matches(event.device)) return;
        void this.connect(event.device);
    };

    private readonly onDisconnect = (event: UsbConnectionEventLike): void => {
        if (!this.link || event.device !== this.link.device) return;
        // Fail the pipes *before* awaiting anything: every pending read and write settles now, so
        // an in-flight transfer's UI reports "unplugged" instead of spinning until a timeout.
        this.link.disconnected();
        const client = this.state.client;
        this.link = null;
        this.publish({ status: "idle", client: null, store: null, info: null, error: null });
        void client?.close();
    };

    private publish(state: DeviceState): void {
        this.state = state;
        for (const listener of this.listeners) listener(state);
    }
}

/** `navigator.usb`, or `null` where the browser has no WebUSB (Firefox, Safari) or no `navigator`. */
export function webUsb(): UsbLike | null {
    const nav = globalThis.navigator as { usb?: UsbLike } | undefined;
    return nav?.usb ?? null;
}

function describe(cause: unknown): string {
    if (cause instanceof Error) return cause.message;
    return String(cause);
}
