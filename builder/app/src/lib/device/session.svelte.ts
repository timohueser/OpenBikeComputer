/**
 * The one device session the page holds, opened once and shared by every surface that writes.
 *
 * Three surfaces (map, route, firmware) talk to one device over one link that allows one transfer
 * at a time, so they cannot each open their own session — the second would be a second
 * `navigator.usb` claim on the same interface. This holder is where the shared one lives.
 *
 * **Opening is not connecting.** C3's `DeviceSession` exists before any device is known, because
 * WebUSB's chooser may only be called from a user gesture: opening the session on mount is what
 * adopts an already-permitted device with no prompt (the "plug it in and the page lights up" path),
 * and `requestDevice()` — called straight out of a click — is what shows the chooser the first
 * time. Neither can substitute for the other.
 */

import { platform } from "../platform";
import type { DeviceSession } from "../usb/session";

/** Reactive holder for the shared session. */
class DeviceHolder {
    /** Non-null once the session has been opened — which says nothing about whether a device is
     *  connected; that is `session.status`. */
    session = $state<DeviceSession | null>(null);
    /** Set only if opening itself failed, which is a page-level fault rather than a device one. */
    error = $state<string | null>(null);

    /**
     * A write that stopped because the device went away, kept **here** rather than where it
     * happened.
     *
     * The moment a device disconnects the whole write panel unmounts — there is no client to
     * render it against — and it would take the failing transfer's error message with it, leaving
     * the rider looking at a Connect button with no account of what happened to their upload. So
     * the sentence outlives its surface, and the next successful connect clears it.
     */
    interrupted = $state<string | null>(null);

    /**
     * Record that a transfer stopped because the link went away. One sentence, written once.
     *
     * Direction-neutral, because both directions end up here: a write that stopped leaves nothing
     * half-written on the card, and a ride pull that stopped leaves nothing partial in the browser
     * (C5 #904 — the object's CRC is only checked once every byte has arrived, so an interrupted
     * pull produces no file at all).
     */
    noteInterrupted(): void {
        this.interrupted =
            "The transfer stopped when the device disconnected. Nothing partial is kept at either " +
            "end — plug it back in and try again.";
    }

    private opening: Promise<DeviceSession | null> | null = null;

    /**
     * Open the session, at most once per page. Safe to call from several components' `onMount`.
     *
     * `openDevice` defaults to the host's own — the parameter exists because the first caller
     * decides, which is how the dev harness (`dev-harness/`) puts a simulated device behind the
     * same UI without the app carrying a reference to one.
     */
    open(openDevice: (() => Promise<DeviceSession>) | null = platform.device): Promise<DeviceSession | null> {
        this.opening ??= this.start(openDevice);
        return this.opening;
    }

    private async start(openDevice: (() => Promise<DeviceSession>) | null): Promise<DeviceSession | null> {
        // Null on a tier without USB at all. The UI never reaches here in that case — `<Gated>` has
        // already replaced the control — so this is a guard, not a branch anyone renders.
        if (!openDevice) return null;
        try {
            const session = await openDevice();
            this.session = session;
            return session;
        } catch (cause) {
            this.error = cause instanceof Error ? cause.message : String(cause);
            return null;
        }
    }
}

export const deviceHolder = new DeviceHolder();
