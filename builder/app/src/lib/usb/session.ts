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

import type { ObjectSource, ProtocolClient } from "./client";
import type { VersionRead } from "./protocol";
import type { DeviceInfo } from "./transport";
import type { DeviceState, DeviceStatus } from "./webusb";

export type { DeviceState, DeviceStatus };

/**
 * Turn a path on **this machine's** disk into an object the transport can send by itself.
 *
 * Present only where both halves of that sentence are true: the host has a filesystem, and the
 * process that owns the USB endpoint can read it. That is the desktop app and nothing else — the
 * implementation is `nativeFileSource` in `lib/desktop/usb.ts`, and the source it returns carries a
 * {@link ObjectSource.sendTo}, so the bytes go disk → endpoint inside Rust without passing through
 * the webview (D4 #909, E3 #913).
 *
 * The path is not a free choice: the backend refuses anything outside the folders the app itself
 * owns (`usb::sendable_path`). What a caller has, in practice, is the path of a map this app just
 * built — which is the whole point of the flow (#894: build a map, plug in, one click).
 */
export type LocalFileSource = (path: string) => Promise<ObjectSource>;

/**
 * A connection to one device, followed over its lifetime.
 *
 * The fields are read reactively by the UI, so an implementation makes them `$state` — the
 * interface only promises they are observable, not how, exactly as `BuildSession` does for builds.
 */
export interface DeviceSession {
    /** Diagnostics only: `"webusb"` on the hosted tier, `"native"` in the desktop app (D4 #909). */
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

    /**
     * Absent on a tier whose transport cannot reach the disk — which is every browser, because a
     * page has no paths. See {@link LocalFileSource}.
     *
     * It hangs off the *session* rather than off `Platform` because the thing it needs is the open
     * link: the backend addresses an endpoint by handle, and the handle changes every time a device
     * is re-plugged. A module-level "current device" would be the same fact kept in a second place,
     * free to disagree with this one.
     */
    readonly localFileSource?: LocalFileSource;
}

/**
 * The framework-free half a session wraps: discovery, hot-plug and the client's lifetime.
 *
 * `WebUsbWatcher` is the browser implementation and `NativeWatcher` (`lib/desktop/usb.ts`) the
 * desktop one, over `nusb`; the reactive shell above them does not change, and neither does any
 * component that renders a session.
 */
export interface DeviceWatcher {
    readonly current: DeviceState;
    subscribe(listener: (state: DeviceState) => void): () => void;
    requestDevice(): Promise<boolean>;
    disconnect(): Promise<void>;
    close(): Promise<void>;
    /** Mirrored onto the session it backs. Absent on `WebUsbWatcher`. */
    localFileSource?(path: string): Promise<ObjectSource>;
}
