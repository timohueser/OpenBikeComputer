/**
 * The WebUSB transport, driven under Node against a scripted `navigator.usb`.
 *
 * The fake is not a stub of the protocol — it is a fake of the *browser API*, with a real
 * {@link MockDevice} behind it. So `WebUsbWatcher.start()`, `openWebUsbLink`, the endpoint
 * discovery and the pipe's transfer translation all run for real, and what the tests assert is the
 * behaviour that only the browser layer can get wrong: the permission model, hot-plug, and settling
 * promptly when the cable comes out.
 *
 * What this cannot cover is silicon. Enumeration, MS OS 2.0 descriptors for Windows' WinUSB
 * binding, real endpoint stalls and actual throughput are all unverified until #889 lands a device
 * that enumerates.
 */

import { describe, expect, it, vi } from "vitest";

import { DeviceError } from "./client";
import { MockDevice, loopbackLink, type LoopbackLink } from "./loopback";
import { ObjectType } from "./protocol";
import {
    OBC_USB_FILTERS,
    WebUsbWatcher,
    discoverLayout,
    openWebUsbLink,
    webUsb,
    type UsbConfigurationLike,
    type UsbConnectionEventLike,
    type UsbDeviceLike,
    type UsbLike,
} from "./webusb";

const VID = OBC_USB_FILTERS[0].vendorId;
const PID = OBC_USB_FILTERS[0].productId!;

/** A vendor interface with the two endpoint pairs the layout rule expects. */
function configuration(packetSize = 64): UsbConfigurationLike {
    return {
        configurationValue: 1,
        interfaces: [
            {
                interfaceNumber: 0,
                alternate: {
                    interfaceClass: 0xff,
                    endpoints: [
                        { endpointNumber: 1, direction: "in", type: "bulk", packetSize },
                        { endpointNumber: 1, direction: "out", type: "bulk", packetSize },
                        { endpointNumber: 2, direction: "in", type: "bulk", packetSize },
                        { endpointNumber: 2, direction: "out", type: "bulk", packetSize },
                    ],
                },
            },
        ],
    };
}

/**
 * A `USBDevice` whose endpoints are wired to a loopback link, with a {@link MockDevice} on the far
 * side. Endpoint 1 is the control pair, endpoint 2 the bulk pair — the layout `discoverLayout`
 * derives.
 */
class FakeUsbDevice implements UsbDeviceLike {
    readonly vendorId: number;
    readonly productId: number;
    readonly serialNumber = "0011223344556677";
    readonly productName = "OpenBikeComputer";

    private open_ = false;
    private config: UsbConfigurationLike | null = null;
    claimed: number | null = null;
    readonly halts: string[] = [];
    private readonly link: LoopbackLink;

    constructor(link: LoopbackLink, vendorId = VID, productId = PID) {
        this.link = link;
        this.vendorId = vendorId;
        this.productId = productId;
    }

    get opened(): boolean {
        return this.open_;
    }

    get configuration(): UsbConfigurationLike | null {
        return this.config;
    }

    async open(): Promise<void> {
        this.open_ = true;
    }

    async close(): Promise<void> {
        this.open_ = false;
    }

    async selectConfiguration(): Promise<void> {
        this.config = configuration();
    }

    async claimInterface(n: number): Promise<void> {
        this.claimed = n;
    }

    async releaseInterface(): Promise<void> {
        this.claimed = null;
    }

    async transferIn(endpointNumber: number, length: number): Promise<{ data?: DataView; status: string }> {
        const pipe = endpointNumber === 1 ? this.link.host.control : this.link.host.bulk;
        const bytes = await pipe.read();
        expect(bytes.length, "a transfer must never exceed the requested length").toBeLessThanOrEqual(length);
        return { status: "ok", data: new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength) };
    }

    async transferOut(endpointNumber: number, data: Uint8Array): Promise<{ bytesWritten: number; status: string }> {
        const pipe = endpointNumber === 1 ? this.link.host.control : this.link.host.bulk;
        await pipe.write(data);
        return { bytesWritten: data.length, status: "ok" };
    }

    async clearHalt(direction: "in" | "out", endpointNumber: number): Promise<void> {
        this.halts.push(`${direction}${endpointNumber}`);
    }
}

/** A `navigator.usb` whose chooser and hot-plug events the test drives. */
class FakeUsb implements UsbLike {
    permitted: UsbDeviceLike[] = [];
    /** What the next `requestDevice` resolves with, or the error it rejects with. */
    chooser: UsbDeviceLike | Error | null = null;
    readonly filtersSeen: Array<Array<{ vendorId?: number; productId?: number }>> = [];

    private readonly listeners = new Map<string, Set<(e: UsbConnectionEventLike) => void>>();

    async getDevices(): Promise<UsbDeviceLike[]> {
        return this.permitted;
    }

    async requestDevice(options: { filters: Array<{ vendorId?: number; productId?: number }> }) {
        this.filtersSeen.push(options.filters);
        if (this.chooser instanceof Error) throw this.chooser;
        if (!this.chooser) {
            const dismissed = new Error("No device selected.");
            dismissed.name = "NotFoundError";
            throw dismissed;
        }
        return this.chooser;
    }

    addEventListener(type: "connect" | "disconnect", listener: (e: UsbConnectionEventLike) => void): void {
        (this.listeners.get(type) ?? this.listeners.set(type, new Set()).get(type)!).add(listener);
    }

    removeEventListener(type: "connect" | "disconnect", listener: (e: UsbConnectionEventLike) => void): void {
        this.listeners.get(type)?.delete(listener);
    }

    emit(type: "connect" | "disconnect", device: UsbDeviceLike): void {
        for (const listener of [...(this.listeners.get(type) ?? [])]) listener({ device });
    }
}

/** A USB host with one OBC on the far end of a loopback. */
function rig(options: Parameters<typeof loopbackLink>[0] = {}) {
    const link = loopbackLink(options);
    const device = new MockDevice(link.device);
    void device.run();
    const usbDevice = new FakeUsbDevice(link);
    const usb = new FakeUsb();
    return { link, device, usbDevice, usb };
}

describe("browser support", () => {
    it("reports no WebUSB rather than pretending", () => {
        // Firefox and Safari take this path. The answer is the desktop app, not a retry — so the
        // state is its own thing, not an error, and the UI can say something true about it.
        const watcher = new WebUsbWatcher({ usb: undefined, ...({} as object) });
        // No `navigator.usb` under Node, so the default lookup finds nothing.
        expect(webUsb()).toBeNull();
        expect(watcher.current.status).toBe("unsupported");
        expect(watcher.current.error).toMatch(/desktop app/);
    });

    it("cannot prompt where there is no API", async () => {
        const watcher = new WebUsbWatcher();
        expect(await watcher.start()).toBe(false);
        expect(await watcher.requestDevice()).toBe(false);
    });
});

describe("the permission model", () => {
    it("adopts an already-permitted device with no prompt at all", async () => {
        // The whole auto-detect story: on every visit after the first, `getDevices()` returns what
        // the user already granted, no gesture required. This is what makes "plug it in and the
        // page lights up" possible, and why `start()` never calls the chooser.
        const { usb, usbDevice } = rig();
        usb.permitted = [usbDevice];
        const watcher = new WebUsbWatcher({ usb });
        expect(await watcher.start()).toBe(true);
        expect(watcher.current.status).toBe("ready");
        expect(watcher.current.identity).toEqual({ version: 2, storeEpoch: 0xa1b2c3d4 });
        expect(watcher.current.info?.firmwareRevision).toBe("0.4.0+abc1234");
        expect(usb.filtersSeen, "adopting must never open the chooser").toEqual([]);
        await watcher.close();
    });

    it("stays idle on a first visit instead of prompting", async () => {
        const { usb } = rig();
        const watcher = new WebUsbWatcher({ usb });
        expect(await watcher.start()).toBe(false);
        expect(watcher.current.status).toBe("idle");
        expect(usb.filtersSeen).toEqual([]);
        await watcher.close();
    });

    it("ignores a permitted device that is not an OBC", async () => {
        const { link, usb } = rig();
        usb.permitted = [new FakeUsbDevice(link, 0x1234, 0x5678)];
        const watcher = new WebUsbWatcher({ usb });
        expect(await watcher.start()).toBe(false);
        await watcher.close();
    });

    it("opens the chooser with the VID/PID filter when asked", async () => {
        const { usb, usbDevice } = rig();
        usb.chooser = usbDevice;
        const watcher = new WebUsbWatcher({ usb });
        await watcher.start();
        expect(await watcher.requestDevice()).toBe(true);
        expect(usb.filtersSeen).toEqual([[{ vendorId: VID, productId: PID }]]);
        expect(watcher.current.status).toBe("ready");
        await watcher.close();
    });

    it("treats a dismissed chooser as a non-event", async () => {
        // The rider closed a dialog. That is not an error to show them.
        const { usb } = rig();
        const watcher = new WebUsbWatcher({ usb });
        await watcher.start();
        expect(await watcher.requestDevice()).toBe(false);
        expect(watcher.current.status).toBe("idle");
        expect(watcher.current.error).toBeNull();
        await watcher.close();
    });
});

describe("hot plug", () => {
    it("connects on a `connect` event", async () => {
        const { usb, usbDevice } = rig();
        const watcher = new WebUsbWatcher({ usb });
        await watcher.start();
        expect(watcher.current.status).toBe("idle");

        const states: string[] = [];
        watcher.subscribe((s) => states.push(s.status));
        usb.emit("connect", usbDevice);
        await vi.waitUntil(() => watcher.current.status === "ready");
        expect(states).toContain("connecting");
        await watcher.close();
    });

    it("returns to idle on a `disconnect`, with no error", async () => {
        const { usb, usbDevice } = rig();
        usb.permitted = [usbDevice];
        const watcher = new WebUsbWatcher({ usb });
        await watcher.start();
        usb.emit("disconnect", usbDevice);
        expect(watcher.current.status).toBe("idle");
        expect(watcher.current.error).toBeNull();
        expect(watcher.current.client).toBeNull();
        await watcher.close();
    });

    it("fails a transfer in flight the moment the cable goes", async () => {
        // #902's acceptance, precisely: unplugging must not leave a spinner. The pipes are failed
        // from the event rather than left for a pending `transferIn` to notice, because a pending
        // one may never settle at all.
        const { usb, usbDevice, device } = rig({ bulkPacketSize: 64 });
        usb.permitted = [usbDevice];
        const watcher = new WebUsbWatcher({ usb });
        await watcher.start();
        const client = watcher.current.client!;

        const bytes = Uint8Array.from({ length: 200_000 }, (_, i) => i & 0xff);
        device.seedRide(
            {
                objectId: 1,
                byteLen: bytes.length,
                startTime: 0,
                distanceM: 0,
                movingTimeS: 0,
                avgSpeedCms: 0,
                climbM: 0,
                name: "big",
            },
            bytes,
        );
        const started = Date.now();
        const download = client.download(ObjectType.Ride, 1, {
            onProgress: (done) => {
                if (done > 512) usb.emit("disconnect", usbDevice);
            },
        });
        const error = await download.catch((e: unknown) => e);
        expect(error).toBeInstanceOf(DeviceError);
        expect((error as DeviceError).code).toBe("link");
        expect(Date.now() - started).toBeLessThan(2_000);
        await watcher.close();
    });
});

describe("the endpoint layout", () => {
    it("takes the lowest IN/OUT pair as control and the next as bulk", () => {
        const layout = discoverLayout(configuration(512));
        expect(layout).toEqual({
            interfaceNumber: 0,
            control: { in: 1, out: 1, packetSize: 512 },
            bulk: { in: 2, out: 2, packetSize: 512 },
        });
    });

    it("explains a device it cannot talk to", () => {
        expect(() => discoverLayout({ configurationValue: 1, interfaces: [] })).toThrow(/vendor-specific/);
        const oneEndpoint: UsbConfigurationLike = {
            configurationValue: 1,
            interfaces: [
                {
                    interfaceNumber: 0,
                    alternate: {
                        interfaceClass: 0xff,
                        endpoints: [{ endpointNumber: 1, direction: "in", type: "bulk", packetSize: 64 }],
                    },
                },
            ],
        };
        expect(() => discoverLayout(oneEndpoint)).toThrow(/two of each/);
    });
});

describe("the pipe", () => {
    it("claims the interface and clears both halves on a reset", async () => {
        const { link, usbDevice } = rig();
        const webusb = await openWebUsbLink(usbDevice);
        expect(usbDevice.claimed).toBe(0);
        await webusb.bulk.reset();
        expect(usbDevice.halts).toEqual(["in2", "out2"]);
        await webusb.close();
        expect(usbDevice.claimed).toBeNull();
        await link.host.close();
    });

    it("refuses a control frame that would not fit one transfer", async () => {
        // A control frame is one USB transfer, so it has to end with a short packet. At exactly the
        // packet size the device could not tell the frame had ended.
        const { link, usbDevice } = rig();
        const webusb = await openWebUsbLink(usbDevice);
        await expect(webusb.control.write(new Uint8Array(64))).rejects.toThrow(/does not fit/);
        await webusb.close();
        await link.host.close();
    });

    it("absorbs a zero-length packet instead of handing back an empty read", async () => {
        // A ZLP is a USB-level marker, not data. Returning it would be indistinguishable from a
        // spurious wakeup for a caller counting bytes towards `total_len`.
        const { link, usbDevice } = rig();
        const real = usbDevice.transferIn.bind(usbDevice);
        let first = true;
        usbDevice.transferIn = async (endpoint: number, length: number) => {
            if (first && endpoint === 2) {
                first = false;
                return { status: "ok", data: new DataView(new ArrayBuffer(0)) };
            }
            return real(endpoint, length);
        };
        const webusb = await openWebUsbLink(usbDevice);
        void link.device.bulk.write(new Uint8Array([1, 2, 3]));
        expect(await webusb.bulk.read()).toEqual(new Uint8Array([1, 2, 3]));
        await webusb.close();
        await link.host.close();
    });
});
