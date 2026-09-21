/**
 * An in-memory cable: two record channels and the EP0 read beside them.
 *
 * This is the **transport** half of the simulated link, and only that. What speaks protocol v4 on
 * the far end is the real engine over a real card (`flat-device.ts`).
 *
 * Two transport properties are modelled on purpose, because a fake that smoothed either one away
 * would hide a bug that only appears on a rider's desk:
 *
 * - **Backpressure**, as a byte high-water mark: a writer that has filled the channel waits for the
 *   reader to drain it.
 * - **Segmentation**: every write is re-sliced to `packetSize`, on both channels, so a record spans
 *   packets exactly as the binding says it may.
 *
 * Timing, stalls and enumeration are not here: those belong to the WebUSB pipe and its own suite.
 */

import { PipeError, throwIfAborted, type BytePipe, type DeviceLink } from "./pipe";
import { encodeDeviceInfo, type DeviceInfo } from "./records";

/** The identity strings a link answers with unless a test names others. */
const DEFAULT_DEVICE_INFO: DeviceInfo = {
    firmwareRevision: "0.4.0+abc1234",
    hardwareRevision: "obc-lm20-r1",
    serialNumber: "0011223344556677",
};

/**
 * One direction of the loopback.
 *
 * **Backpressure** is a byte high-water mark. Real backpressure comes from the device NAKing an
 * endpoint it has not drained, and a client that queued writes without ever retiring them would
 * outrun any real device. Faking it here is what makes that bug fail in CI.
 *
 * **Segmentation**: every write is re-sliced to `packetSize`, on *both* channels. A record may span
 * packets on either pair, so a channel that kept writes whole would be modelling a transport
 * property USB does not have.
 */
class Channel {
    private readonly chunks: Uint8Array[] = [];
    private queued = 0;
    private readers: Array<{ resolve: (v: Uint8Array) => void; reject: (e: unknown) => void }> = [];
    private writers: Array<() => void> = [];
    private closed = false;

    constructor(
        private readonly packetSize: number,
        private readonly highWaterMark: number,
    ) {}

    /** Bytes waiting to be read — the backpressure gauge tests assert on. */
    get depth(): number {
        return this.queued;
    }

    async push(bytes: Uint8Array, signal?: AbortSignal): Promise<void> {
        if (this.closed) throw new PipeError("closed", "The loopback pipe is closed.");
        throwIfAborted(signal, "the write");
        // Copy: the caller may reuse its buffer the moment this resolves, and a queued view of a
        // recycled buffer is the classic way a transfer arrives corrupted.
        const owned = bytes.slice();
        for (let at = 0; at < owned.length; at += this.packetSize) {
            this.enqueue(owned.subarray(at, Math.min(at + this.packetSize, owned.length)));
        }
        // A writer parked on the high-water mark has to stay cancellable: a cancelled upload whose
        // last write is waiting for room would otherwise never observe the abort, and the caller
        // would hang exactly where the UI promised a Cancel button.
        while (this.queued > this.highWaterMark && !this.closed) {
            throwIfAborted(signal, "the write");
            await new Promise<void>((resolve) => {
                this.writers.push(resolve);
                signal?.addEventListener("abort", () => resolve(), { once: true });
            });
        }
        throwIfAborted(signal, "the write");
        if (this.closed) throw new PipeError("closed", "The loopback pipe closed while writing.");
    }

    private enqueue(slice: Uint8Array): void {
        const reader = this.readers.shift();
        if (reader) {
            reader.resolve(slice);
            return;
        }
        this.chunks.push(slice);
        this.queued += slice.length;
    }

    pull(signal?: AbortSignal): Promise<Uint8Array> {
        throwIfAborted(signal, "the read");
        const next = this.chunks.shift();
        if (next) {
            this.queued -= next.length;
            this.wake();
            return Promise.resolve(next);
        }
        if (this.closed) return Promise.reject(new PipeError("closed", "The loopback pipe is closed."));
        return new Promise<Uint8Array>((resolve, reject) => {
            const entry = {
                resolve: (v: Uint8Array) => {
                    signal?.removeEventListener("abort", onAbort);
                    resolve(v);
                },
                reject: (e: unknown) => {
                    signal?.removeEventListener("abort", onAbort);
                    reject(e);
                },
            };
            const onAbort = () => {
                this.readers = this.readers.filter((r) => r !== entry);
                reject(new PipeError("aborted", "The read was cancelled.", { cause: signal?.reason }));
            };
            signal?.addEventListener("abort", onAbort, { once: true });
            this.readers.push(entry);
        });
    }

    /** Drop everything queued and release blocked writers — the pipe-reset primitive. */
    clear(): void {
        this.chunks.length = 0;
        this.queued = 0;
        this.wake();
    }

    close(): void {
        if (this.closed) return;
        this.closed = true;
        const error = new PipeError("closed", "The loopback pipe is closed.");
        while (this.readers.length) this.readers.shift()?.reject(error);
        this.wake();
    }

    private wake(): void {
        const waiting = this.writers;
        this.writers = [];
        for (const resume of waiting) resume();
    }
}

/** One end of a loopback: reads from `inbound`, writes to `outbound`. */
class LoopbackPipe implements BytePipe {
    readonly transport = "loopback";

    constructor(
        private readonly inbound: Channel,
        private readonly outbound: Channel,
    ) {}

    private isOpen = true;

    get open(): boolean {
        return this.isOpen;
    }

    /** Bytes waiting for this end to read. */
    get depth(): number {
        return this.inbound.depth;
    }

    read(signal?: AbortSignal): Promise<Uint8Array> {
        return this.inbound.pull(signal);
    }

    write(bytes: Uint8Array, signal?: AbortSignal): Promise<void> {
        return this.outbound.push(bytes, signal);
    }

    /**
     * Return this end to a known state — and **only the end this side owns**.
     *
     * `inbound` is what has been delivered *to* this side and nobody read; dropping it is what a host
     * does when it walks away from a transfer.
     *
     * `outbound` is emphatically **not** cleared. On the host side it models bytes already handed to
     * the transport, and `reset()` there is `clearHalt`, which cancels no transfer and un-queues no
     * byte. Clearing it here would make every stray-byte scenario self-healing in tests and
     * self-healing nowhere else.
     */
    async reset(): Promise<void> {
        this.inbound.clear();
    }

    async close(): Promise<void> {
        this.isOpen = false;
        this.inbound.close();
        this.outbound.close();
    }
}

/** Tuning for {@link loopbackLink}. The defaults mirror a high-speed USB interface. */
export interface LoopbackOptions {
    /** Slice size handed to a reader on both channels. 512 = a high-speed bulk endpoint's max packet. */
    packetSize?: number;
    /** Bytes a writer may have outstanding on the stream channel before it blocks. */
    streamHighWaterMark?: number;
}

/** The two ends of a link: `host` for the client, `device` for the simulated device. */
export interface LoopbackLink {
    host: DeviceLink;
    device: DeviceLink;
    /** Bytes queued but unread on the stream channel in one direction — the backpressure gauge. */
    streamDepth(direction: "to-device" | "to-host"): number;
}

/**
 * Two record channels and the EP0 read beside them.
 *
 * `vendorIn` is on the **host** link only, because that is the direction the request is defined in:
 * the host asks and the device answers. The device end has no such method and needs none.
 */
export function loopbackLink(options: LoopbackOptions & { deviceInfo?: DeviceInfo } = {}): LoopbackLink {
    const { packetSize = 512, streamHighWaterMark = 64 * 1024 } = options;
    const hostToDeviceControl = new Channel(packetSize, 16 * 1024);
    const deviceToHostControl = new Channel(packetSize, 16 * 1024);
    const hostToDeviceStream = new Channel(packetSize, streamHighWaterMark);
    const deviceToHostStream = new Channel(packetSize, streamHighWaterMark);
    const info = options.deviceInfo ?? DEFAULT_DEVICE_INFO;

    const host: DeviceLink = {
        control: new LoopbackPipe(deviceToHostControl, hostToDeviceControl),
        stream: new LoopbackPipe(deviceToHostStream, hostToDeviceStream),
        async vendorIn(request: number, _value: number, length: number) {
            // Modelled to the letter, short transfer included: a host that assumed it got `length`
            // bytes back would work here and fail on glass.
            if (request !== 0x20) throw new PipeError("device-error", `the device stalled vendor request ${request}.`);
            return encodeDeviceInfo(info).subarray(0, length);
        },
        async close() {
            await this.control.close();
            await this.stream.close();
        },
    };
    const device: DeviceLink = {
        control: new LoopbackPipe(hostToDeviceControl, deviceToHostControl),
        stream: new LoopbackPipe(hostToDeviceStream, deviceToHostStream),
        async close() {
            await this.control.close();
            await this.stream.close();
        },
    };
    return {
        host,
        device,
        streamDepth: (direction) => (direction === "to-device" ? hostToDeviceStream : deviceToHostStream).depth,
    };
}
