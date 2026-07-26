/**
 * `DeviceSession` — what the rest of the app holds when it wants to talk to an OBC.
 *
 * This is the type `Platform.device()` hands back (A1 #895 left it as a named placeholder for this
 * issue). It is a *session*, not a *device*, and the difference is forced by the browser: WebUSB's
 * chooser may only open from a user gesture, so a session has to exist — observable, with a status
 * and a way to prompt — before any device is known. A `device()` that resolved only once something
 * was connected could never be called from page load, which is exactly when the auto-detect path
 * has to run.
 *
 * So the lifecycle the UI renders is:
 *
 * 1. `idle` — no device. C4/C5 show a "Connect your OBC" button; clicking it calls
 *    {@link DeviceSession.requestDevice} from inside the gesture.
 * 2. `connecting` — a device was adopted or chosen; identity is being read.
 * 3. `ready` — {@link DeviceSession.client} is live. Writes and reads are available.
 * 4. `error` — {@link DeviceSession.error} is a sentence written for a rider.
 * 5. `unsupported` — no WebUSB in this browser. Not a failure to retry: the answer is the desktop
 *    app, and the UI should say so rather than offering a button that cannot work.
 *
 * Unplugging returns the session to `idle` with no error — a rider pulling a cable is not a fault —
 * and re-plugging a permitted device reconnects on its own.
 *
 * ## Where this stops and the gating layer starts
 *
 * C2 (#901) owns whether a USB affordance is offered at all, through `<Gated>` and the platform's
 * `caps.deviceUsb` / `usbViaWebUsb` flags — `need={["deviceUsb", "webUsb"]}`, in that order, so a
 * Safari visitor and a visitor on a tier without USB get different sentences. **Do not gate on this
 * session's `supported` flag instead**: it is a runtime probe of `navigator.usb`, and the desktop
 * app's webview has no WebUSB while its tier reaches USB natively (D4 #909), so probing would
 * disable the very tier that works best. `supported` exists to explain a session that cannot
 * connect, not to decide whether the button is drawn.
 */

import type { ProtocolClient } from "./client";
import type { VersionRead } from "./protocol";
import type { DeviceInfo } from "./transport";
import type { DeviceState, DeviceStatus } from "./webusb";

export type { DeviceState, DeviceStatus };

/**
 * A connection to one device, followed over its lifetime.
 *
 * The fields are read reactively by the UI, so an implementation makes them `$state` — the
 * interface only promises they are observable, not how, exactly as `BuildSession` does for builds.
 */
export interface DeviceSession {
    /** Diagnostics only: `"webusb"` today, `"native"` when D4 (#909) lands. */
    readonly transport: string;
    /** False where the browser has no WebUSB at all. Distinct from "nothing plugged in". */
    readonly supported: boolean;
    readonly status: DeviceStatus;
    /** Non-null exactly when `status === "ready"`. */
    readonly client: ProtocolClient | null;
    /** The protocol version and store epoch, read on connect. Id-keyed state scopes to the epoch. */
    readonly identity: VersionRead | null;
    /** The running firmware version, board id and serial — what "update available" compares to. */
    readonly info: DeviceInfo | null;
    readonly error: string | null;

    /**
     * Open the browser's device chooser.
     *
     * **Call this synchronously from a user gesture.** Not after an `await`, not from a timer: the
     * browser checks that the call stack came from a real click. Resolves `false` when the rider
     * dismisses the chooser, which is a normal outcome and not an error state.
     */
    requestDevice(): Promise<boolean>;

    /** Drop the device but keep watching, so plugging it back in reconnects. */
    disconnect(): Promise<void>;

    /** Stop watching and release everything. */
    close(): Promise<void>;
}

/**
 * The framework-free half a session wraps: discovery, hot-plug and the client's lifetime.
 *
 * `WebUsbWatcher` is the browser implementation; D4's native transport implements the same three
 * methods over `nusb`, and the reactive shell above it does not change.
 */
export interface DeviceWatcher {
    readonly current: DeviceState;
    subscribe(listener: (state: DeviceState) => void): () => void;
    requestDevice(): Promise<boolean>;
    disconnect(): Promise<void>;
    close(): Promise<void>;
}
