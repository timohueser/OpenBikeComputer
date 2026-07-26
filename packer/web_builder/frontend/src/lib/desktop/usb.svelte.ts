/**
 * The desktop host's device session — the reactive shell C3 already wrote, over a native watcher.
 *
 * This file is three lines of substance on purpose. `WatchedDeviceSession` mirrors any
 * `DeviceWatcher`'s snapshots into runes, and `NativeWatcher` has the same three methods
 * `WebUsbWatcher` does, so the entire desktop USB session is "the same session, different
 * transport". If this file ever needs to grow a branch, the seam has stopped holding.
 */

import { WatchedDeviceSession } from "../usb/session.svelte";
import type { DeviceSession } from "../usb/session";
import { NativeWatcher, type NativeWatcherOptions } from "./usb";

/**
 * Open a native USB session and adopt an attached device if there is one.
 *
 * Returns as soon as the adopt attempt has settled — with a live client when a device was found and
 * `idle` when none was. Neither is a failure: the session's job is to be something the UI can
 * render in every state, including "nothing is plugged in yet".
 *
 * There is no `unsupported` state to reach here. That one means "this browser has no WebUSB", and
 * not having to say it is the reason this tier exists (#894).
 */
export async function openNativeSession(options: NativeWatcherOptions = {}): Promise<DeviceSession> {
    const watcher = new NativeWatcher(options);
    const session = new WatchedDeviceSession(watcher, "native");
    await watcher.start();
    return session;
}
