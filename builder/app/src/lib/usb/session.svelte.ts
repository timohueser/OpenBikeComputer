/**
 * The reactive shell over a {@link DeviceWatcher} — the one Svelte-aware file in `lib/usb/`.
 *
 * Everything with logic in it (pipes, codecs, the client, discovery, the mock device) is plain
 * TypeScript, tested under Node and reusable by the desktop app. This file exists only to mirror a
 * watcher's snapshots into runes so a component can read `session.status` and re-render, mirroring
 * how `JobTracker` shells the build API.
 */

import { WebUsbWatcher, type WatcherOptions } from "./webusb";
import type { DeviceSession, DeviceStatus, DeviceWatcher, LocalFileSource } from "./session";
import type { ProtocolClient } from "./client";
import type { VersionRead } from "./protocol";
import type { DeviceInfo } from "./transport";

export class WatchedDeviceSession implements DeviceSession {
    status = $state<DeviceStatus>("idle");
    client = $state<ProtocolClient | null>(null);
    identity = $state<VersionRead | null>(null);
    info = $state<DeviceInfo | null>(null);
    error = $state<string | null>(null);

    readonly transport: string;
    readonly supported: boolean;
    /**
     * Present exactly when the watcher has one, so the "is there a disk-to-endpoint path" question
     * has a single answer and it is the transport's. Bound to the watcher rather than copied, so it
     * keeps reading whichever link is open now.
     */
    readonly localFileSource?: LocalFileSource;

    private readonly watcher: DeviceWatcher;
    private readonly unsubscribe: () => void;

    constructor(watcher: DeviceWatcher, transport: string) {
        this.watcher = watcher;
        this.transport = transport;
        this.supported = watcher.current.status !== "unsupported";
        const local = watcher.localFileSource;
        if (local) this.localFileSource = (path) => local.call(watcher, path);
        // `subscribe` fires immediately with the current snapshot, so the initial state is set here
        // rather than duplicated above.
        this.unsubscribe = watcher.subscribe((state) => {
            this.status = state.status;
            this.client = state.client;
            this.identity = state.identity;
            this.info = state.info;
            this.error = state.error;
        });
    }

    requestDevice(): Promise<boolean> {
        return this.watcher.requestDevice();
    }

    disconnect(): Promise<void> {
        return this.watcher.disconnect();
    }

    async close(): Promise<void> {
        this.unsubscribe();
        await this.watcher.close();
    }
}

/**
 * Open a WebUSB session and adopt an already-permitted device if one is plugged in.
 *
 * Returns as soon as the adopt attempt has settled — with a live client when a device was found,
 * `idle` when none was, and `unsupported` in a browser without WebUSB. None of those is a failure,
 * because the session's whole job is to be something the UI can render in every one of them.
 */
export async function openWebUsbSession(options: WatcherOptions = {}): Promise<DeviceSession> {
    const watcher = new WebUsbWatcher(options);
    const session = new WatchedDeviceSession(watcher, "webusb");
    await watcher.start();
    return session;
}
