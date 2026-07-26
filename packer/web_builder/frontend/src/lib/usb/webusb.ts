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
 * ## What is provisional
 *
 * The VID/PID and the endpoint layout are #889's to settle. Both are options here with documented
 * defaults, so adopting the real values is a constant change.
 */

import { PipeError, withAbort, type BytePipe, type DeviceLink } from "./pipe";
import { ProtocolClient, type ClientOptions } from "./client";
import type { DeviceInfo } from "./transport";
import type { VersionRead } from "./protocol";

// --- the slice of WebUSB this file uses ---------------------------------------
//
// Declared structurally rather than pulled from `@types/w3c-web-usb`, for two reasons: it documents
// exactly which of the API's surface the transport depends on, and it makes the whole thing
// injectable, so `webusb.test.ts` drives the real code paths under Node with a scripted device.

export interface UsbEndpointLike {
    endpointNumber: number;
    direction: "in" | "out";
    type: string;
    packetSize: number;
}

export interface UsbInterfaceLike {
    interfaceNumber: number;
    alternate: { interfaceClass: number; endpoints: UsbEndpointLike[] };
}

export interface UsbConfigurationLike {
    configurationValue: number;
    interfaces: UsbInterfaceLike[];
}

export interface UsbDeviceLike {
    readonly vendorId: number;
    readonly productId: number;
    readonly serialNumber?: string;
    readonly productName?: string;
    readonly opened: boolean;
    readonly configuration: UsbConfigurationLike | null;
    open(): Promise<void>;
    close(): Promise<void>;
    selectConfiguration(value: number): Promise<void>;
    claimInterface(interfaceNumber: number): Promise<void>;
    releaseInterface(interfaceNumber: number): Promise<void>;
    transferIn(endpointNumber: number, length: number): Promise<{ data?: DataView; status: string }>;
    transferOut(endpointNumber: number, data: Uint8Array): Promise<{ bytesWritten: number; status: string }>;
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
 * says "not allocated yet". #889 owns the real allocation, and until it lands a filter that
 * pretended to be final would be worse than one that admits it isn't.
 */
export const OBC_USB_FILTERS: ReadonlyArray<{ vendorId: number; productId?: number }> = [
    { vendorId: 0x1209, productId: 0x0001 },
];

/** USB vendor-specific interface class — the class a WebUSB-reachable interface must use. */
const VENDOR_CLASS = 0xff;

// --- the pipe -----------------------------------------------------------------

/** One endpoint pair as a byte pipe. */
class WebUsbPipe implements BytePipe {
    readonly transport = "webusb";

    private failure: PipeError | null = null;
    private closedByUs = false;
    /** Rejectors for {@link dead}, released when the pipe closes or fails. */
    private readonly mourners: Array<(error: PipeError) => void> = [];

    constructor(
        readonly kind: "control" | "bulk",
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
            const result = await this.transfer(
                () => this.device.transferIn(this.inEndpoint, this.packetSize),
                signal,
                "the read",
            );
            if (result.status === "stall") {
                throw new PipeError("device-error", `The device stalled endpoint ${this.inEndpoint}.`);
            }
            const bytes = result.data ? new Uint8Array(result.data.buffer, result.data.byteOffset, result.data.byteLength) : null;
            // A zero-length packet is a USB-level marker, not data; a caller that got an empty array
            // could not tell it from a spurious wakeup, so absorb it and wait for real bytes.
            if (bytes && bytes.length > 0) return bytes.slice();
        }
    }

    async write(bytes: Uint8Array, signal?: AbortSignal): Promise<void> {
        this.check();
        // A control frame is one transfer, so it has to fit in one packet: at exactly the packet
        // size the device could not tell the frame had ended without a zero-length packet, and the
        // frames this protocol sends are all far below it (the longest is a 512-byte Config write
        // on an endpoint sized for it).
        if (this.kind === "control" && bytes.length >= this.packetSize) {
            throw new PipeError(
                "device-error",
                `a ${bytes.length}-byte control frame does not fit the ${this.packetSize}-byte endpoint.`,
            );
        }
        const result = await this.transfer(() => this.device.transferOut(this.outEndpoint, bytes), signal, "the write");
        if (result.status !== "ok") {
            throw new PipeError("device-error", `The device answered "${result.status}" to a write.`);
        }
    }

    /**
     * Clear both halves of the endpoint pair.
     *
     * `clearHalt` is what un-sticks a stalled endpoint and, on the device side, what a firmware
     * treats as "the host has given up on this exchange" — the closest USB equivalent of BLE's
     * close-and-reopen-the-CoC (§4.1). It is best-effort: an endpoint that was never halted may
     * reject the request, and that is not a failure of the reset.
     */
    async reset(): Promise<void> {
        this.check();
        for (const [direction, endpoint] of [
            ["in", this.inEndpoint],
            ["out", this.outEndpoint],
        ] as const) {
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

    /** Run one WebUSB transfer, translating its failures and honouring cancellation. */
    private async transfer<T>(run: () => Promise<T>, signal: AbortSignal | undefined, what: string): Promise<T> {
        let promise: Promise<T>;
        try {
            promise = run();
        } catch (cause) {
            throw this.asPipeError(cause);
        }
        try {
            // An orphaned transfer is left to settle on its own: WebUSB cannot cancel a submitted
            // one. That is safe only because a cancelled transfer is always followed by a reset —
            // the bytes it eventually delivers are bytes the caller has agreed to discard.
            return await Promise.race([withAbort(promise, signal, what, () => undefined), this.dead()]);
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
    bulk: { in: number; out: number; packetSize: number };
}

/**
 * Pick the vendor interface and split its endpoints into a control pair and a bulk pair.
 *
 * The rule — lowest-numbered IN/OUT pair is control, the next is bulk — is the host's half of a
 * contract #889 has yet to write down. It is deliberately mechanical so the firmware descriptor can
 * be read off it, and {@link openWebUsbLink} takes an explicit layout for when the real one differs.
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
                "two of each are needed (a control pair and a bulk pair).",
        );
    }
    return {
        interfaceNumber: iface.interfaceNumber,
        control: { in: ins[0].endpointNumber, out: outs[0].endpointNumber, packetSize: ins[0].packetSize },
        bulk: { in: ins[1].endpointNumber, out: outs[1].endpointNumber, packetSize: ins[1].packetSize },
    };
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

/** Open, configure and claim a device, returning its two pipes. */
export async function openWebUsbLink(device: UsbDeviceLike, layout?: EndpointLayout): Promise<WebUsbLink> {
    if (!device.opened) await device.open();
    if (!device.configuration) await device.selectConfiguration(1);
    const configuration = device.configuration;
    if (!configuration) throw new PipeError("device-error", "The device offers no USB configuration.");
    const chosen = layout ?? discoverLayout(configuration);
    await device.claimInterface(chosen.interfaceNumber);

    const control = new WebUsbPipe("control", device, chosen.control.in, chosen.control.out, chosen.control.packetSize);
    const bulk = new WebUsbPipe("bulk", device, chosen.bulk.in, chosen.bulk.out, chosen.bulk.packetSize);
    return {
        device,
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

/** The watcher's whole observable state, handed to subscribers as one immutable snapshot. */
export interface DeviceState {
    status: DeviceStatus;
    /** Non-null exactly when `status === "ready"`. */
    client: ProtocolClient | null;
    identity: VersionRead | null;
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
 * Finds the device, keeps up with plugging and unplugging, and owns the {@link ProtocolClient}.
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
            identity: null,
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
        this.publish({ status: this.usb ? "idle" : "unsupported", client: null, identity: null, info: null, error: null });
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
        try {
            const link = await openWebUsbLink(device, this.options.layout);
            const client = new ProtocolClient(link, { timeoutMs: this.options.timeoutMs });
            // The identity read is first on every connection and gates everything else: a version
            // mismatch is surfaced and stopped on rather than best-effort decoded (§1).
            const identity = await client.identity();
            const info = await client.deviceInfo();
            this.link = link;
            this.publish({ status: "ready", client, identity, info, error: null });
            return true;
        } catch (cause) {
            this.link = null;
            this.publish({ status: "error", client: null, identity: null, info: null, error: describe(cause) });
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
        this.publish({ status: "idle", client: null, identity: null, info: null, error: null });
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
