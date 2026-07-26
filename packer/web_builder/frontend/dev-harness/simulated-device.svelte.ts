/**
 * A {@link DeviceSession} backed by the simulated device — **the dev harness only**.
 *
 * The LM20's USB peripheral does not exist yet (#889), so without this there is no way to click
 * through a map upload, a route drop or a firmware update at all. `loopback.ts` already models the
 * protocol properly (id assignment, dedup, `busy`, the abort handshake, packet-sized bulk reads),
 * so wiring it to a session drives the real UI against a real protocol conversation — the only
 * fiction is the cable.
 *
 * **Why it lives outside `src/`.** C3 drew a hard line: no shipping module may import
 * `lib/usb/loopback`, guarded twice — a source scan in `usb/vectors.test.ts` and a chunk assertion
 * in `usb/bundle.test.ts`. A dev-only dynamic import inside `src/` would satisfy neither, and the
 * chunk guard is right to refuse it: whether such a branch is tree-shaken depends on how the build
 * was invoked (`import.meta.env.DEV` is not `false` when Rollup runs under vitest), so "it gets
 * dropped in production" would be a property nothing in CI actually checks. A separate entry point
 * that no tier's build has as an input is a fact instead of a hope.
 */

import { ProtocolClient } from "../src/lib/usb/client";
import { WatchedDeviceSession } from "../src/lib/usb/session.svelte";
import { MockDevice, loopbackLink } from "../src/lib/usb/loopback";
import type { BytePipe, DeviceLink } from "../src/lib/usb/pipe";
import type { DeviceSession, DeviceState, DeviceWatcher } from "../src/lib/usb/session";

const IDLE: DeviceState = { status: "idle", client: null, identity: null, info: null, error: null };

/**
 * The rate the simulated device drains its bulk endpoint at.
 *
 * ~700 KB/s is the measured ceiling of the real thing: SPI to the SD card at the proven 8 MHz, not
 * anything about USB (#889, and `sd-read-speed-levers`). An unthrottled loopback finishes a 100 MB
 * "map" in seconds, which would make every progress bar, rate and remaining-time estimate in the
 * UI untestable — and those exist precisely because the real transfer takes minutes.
 */
const CARD_BYTES_PER_SECOND = 700 * 1024;

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/** The device end of a link, paced to {@link CARD_BYTES_PER_SECOND}. */
function paced(link: DeviceLink): DeviceLink {
    const bulk = link.bulk;
    let startedAt = 0;
    let seen = 0;
    const throttledBulk: BytePipe = {
        transport: bulk.transport,
        get open() {
            return bulk.open;
        },
        async read(signal) {
            const slice = await bulk.read(signal);
            if (!startedAt) startedAt = performance.now();
            seen += slice.length;
            // Pace against the whole transfer rather than sleeping per packet: a 512-byte packet
            // is 0.7 ms of card time, well under a timer's resolution.
            const wait = startedAt + (seen / CARD_BYTES_PER_SECOND) * 1000 - performance.now();
            if (wait > 5) await sleep(wait);
            return slice;
        },
        write: (bytes, signal) => bulk.write(bytes, signal),
        reset: () => {
            startedAt = 0;
            seen = 0;
            return bulk.reset();
        },
        close: () => bulk.close(),
    };
    return { control: link.control, bulk: throttledBulk, close: () => link.close() };
}

/** A watcher whose "device" is an in-memory one. The same three methods the WebUSB watcher has. */
class LoopbackWatcher implements DeviceWatcher {
    private state: DeviceState = IDLE;
    private readonly listeners = new Set<(state: DeviceState) => void>();
    private open: { device: MockDevice; close: () => Promise<void> } | null = null;

    get current(): DeviceState {
        return this.state;
    }

    subscribe(listener: (state: DeviceState) => void): () => void {
        this.listeners.add(listener);
        listener(this.state);
        return () => this.listeners.delete(listener);
    }

    /** Stands in for the browser's chooser, so the connect button is exercised exactly as it will
     *  be against hardware — including the rule that it only runs from a real click. */
    async requestDevice(): Promise<boolean> {
        if (this.open) return true;
        this.publish({ ...IDLE, status: "connecting" });
        const link = loopbackLink();
        const device = new MockDevice(paced(link.device));
        void device.run();
        const client = new ProtocolClient(link.host);
        this.open = {
            device,
            close: async () => {
                device.stop();
                await client.close();
                await link.device.close();
            },
        };
        const identity = await client.identity();
        const info = await client.deviceInfo();
        this.publish({ status: "ready", client, identity, info, error: null });
        return true;
    }

    async disconnect(): Promise<void> {
        const open = this.open;
        this.open = null;
        await open?.close();
        this.publish(IDLE);
    }

    async close(): Promise<void> {
        await this.disconnect();
        this.listeners.clear();
    }

    private publish(state: DeviceState): void {
        this.state = state;
        for (const listener of this.listeners) listener(state);
    }
}

/** Open a session over the simulated device. Nothing is connected until `requestDevice()`. */
export async function openSimulatedSession(): Promise<DeviceSession> {
    return new WatchedDeviceSession(new LoopbackWatcher(), "loopback");
}
