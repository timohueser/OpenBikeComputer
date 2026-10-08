/**
 * The WebUSB transport, driven under Node against a scripted `navigator.usb`.
 *
 * The fake is a fake of the *browser API*, not of the protocol, with the real {@link FlatDevice}
 * behind it. So `WebUsbWatcher.start()`, `openWebUsbLink`, the endpoint discovery, the record
 * framing and the pipe's transfer translation all run for real, and what is asserted is what only
 * the browser layer can get wrong: the permission model, the descriptor match that settles the
 * USB-binding major, hot-plug, and settling promptly when the cable comes out.
 *
 * A record may span USB packets, so a transfer is *not* a frame in either direction. The loopback
 * under the fake device re-slices every write to its packet size, so a reader that assumed one
 * transfer was one record fails here rather than on glass.
 *
 * Silicon is out of reach: enumeration, MS OS 2.0 descriptors, real endpoint stalls and throughput
 * are unverified until a device that enumerates exists.
 */

import { beforeAll, describe, expect, it, vi } from "vitest";

import { loadFlatDevice } from "../../../test-support/flat-device/load";
import { DeviceError, FlatStoreClient } from "./client";
import { FlatDevice, streamCeilingFor, type FlatDeviceOptions } from "./flat-device";
import { loopbackLink, type LoopbackLink, type LoopbackOptions } from "./loopback";
import {
    DEVICE_INFO_MAX,
    GET_DEVICE_INFO,
    MAX_HOST_CONTROL_RECORD,
    MAX_HOST_STREAM_RECORD,
    RecordChannel,
    USB_BINDING_MAJOR,
    decodeDeviceInfo,
    frameRecord,
    type DeviceInfo,
} from "./records";
import { HEAD_REVISION, ObjectKind } from "./protocol";
import {
    OBC_USB_FILTERS,
    WebUsbWatcher,
    checkUsbBindingMajor,
    discoverLayout,
    openWebUsbLink,
    webUsb,
    type UsbConfigurationLike,
    type UsbConnectionEventLike,
    type UsbControlInResult,
    type UsbControlSetup,
    type UsbDeviceLike,
    type UsbLike,
} from "./webusb";

const VID = OBC_USB_FILTERS[0].vendorId;
const PID = OBC_USB_FILTERS[0].productId!;

/**
 * A vendor interface with the two endpoint pairs the layout rule expects. 512 bytes is the LM20's
 * real number, and it matters here only as the length a `transferIn` asks for.
 */
function configuration(
    options: { packetSize?: number; interfaceProtocol?: number | null; interfaceNumber?: number } = {},
): UsbConfigurationLike {
    const packetSize = options.packetSize ?? 512;
    // Defaults to **2**, not 0. The claimed interface number goes in `wIndex`, and a rig that always
    // used interface 0 made that assertion unfalsifiable. The device really does enumerate interface
    // 0 today; the point of the rig is to be able to tell the difference.
    const interfaceNumber = options.interfaceNumber ?? 2;
    const protocol = options.interfaceProtocol === undefined ? USB_BINDING_MAJOR : options.interfaceProtocol;
    return {
        configurationValue: 1,
        interfaces: [
            {
                interfaceNumber,
                alternate: {
                    interfaceClass: 0xff,
                    // `null` models a descriptor that states nothing — the field is optional.
                    ...(protocol === null ? {} : { interfaceProtocol: protocol }),
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

/** How a {@link FakeUsbDevice} presents itself, before a byte moves. */
interface FakeDeviceOptions {
    vendorId?: number;
    productId?: number;
    /** `bcdDevice`'s high byte. `null` models a device that states nothing there. */
    deviceVersionMajor?: number | null;
    /** `bInterfaceProtocol`. `null` models a descriptor that states nothing there. */
    interfaceProtocol?: number | null;
    packetSize?: number;
}

/**
 * A `USBDevice` whose endpoints are wired to a loopback link, with a {@link FlatDevice} on the far
 * side. Endpoint 1 is the control pair, endpoint 2 the stream pair — the layout the firmware
 * descriptors declare.
 */
class FakeUsbDevice implements UsbDeviceLike {
    readonly vendorId: number;
    readonly productId: number;
    readonly deviceVersionMajor?: number;
    readonly serialNumber = "0011223344556677";
    readonly productName = "OpenBikeComputer";

    private open_ = false;
    private config: UsbConfigurationLike | null = null;
    claimed: number | null = null;
    readonly halts: string[] = [];
    /** Every `transferIn` this device served, so a test can count packets rather than assume them. */
    reads = 0;
    /** Every EP0 setup packet, so the request can be asserted rather than described. */
    readonly setups: Array<{ setup: UsbControlSetup; length: number }> = [];
    /** Overrides the EP0 answer, for the two shapes the loopback would never produce. */
    controlAnswer: ((setup: UsbControlSetup, length: number) => Promise<UsbControlInResult>) | null = null;

    private readonly link: LoopbackLink;
    private readonly options: FakeDeviceOptions;

    constructor(link: LoopbackLink, options: FakeDeviceOptions = {}) {
        this.link = link;
        this.options = options;
        this.vendorId = options.vendorId ?? VID;
        this.productId = options.productId ?? PID;
        const major = options.deviceVersionMajor === undefined ? USB_BINDING_MAJOR : options.deviceVersionMajor;
        if (major !== null) this.deviceVersionMajor = major;
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
        this.config = configuration({
            packetSize: this.options.packetSize,
            interfaceProtocol: this.options.interfaceProtocol,
        });
    }

    async claimInterface(n: number): Promise<void> {
        this.claimed = n;
    }

    async releaseInterface(): Promise<void> {
        this.claimed = null;
    }

    async transferIn(endpointNumber: number, length: number): Promise<{ data?: DataView; status: string }> {
        const pipe = endpointNumber === 1 ? this.link.host.control : this.link.host.stream;
        const bytes = await pipe.read();
        this.reads += 1;
        expect(bytes.length, "a transfer must never exceed the requested length").toBeLessThanOrEqual(length);
        return { status: "ok", data: new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength) };
    }

    async transferOut(endpointNumber: number, data: Uint8Array): Promise<{ bytesWritten: number; status: string }> {
        const pipe = endpointNumber === 1 ? this.link.host.control : this.link.host.stream;
        await pipe.write(data);
        return { bytesWritten: data.length, status: "ok" };
    }

    /**
     * The EP0 read, answered by the loopback's own `vendorIn`, so the payload a test decodes is the
     * one `encodeDeviceInfo` produced rather than a shape invented here.
     */
    async controlTransferIn(setup: UsbControlSetup, length: number): Promise<UsbControlInResult> {
        this.setups.push({ setup, length });
        if (this.controlAnswer) return this.controlAnswer(setup, length);
        const payload = await this.link.host.vendorIn!(setup.request, setup.value, length);
        return { status: "ok", data: new DataView(payload.buffer, payload.byteOffset, payload.byteLength) };
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
function rig(
    options: LoopbackOptions & { deviceInfo?: DeviceInfo } = {},
    deviceOptions: FlatDeviceOptions = {},
    fake: FakeDeviceOptions = {},
) {
    const link = loopbackLink(options);
    const device = new FlatDevice(link.device, deviceOptions);
    void device.run();
    const usbDevice = new FakeUsbDevice(link, fake);
    const usb = new FakeUsb();
    return { link, device, usbDevice, usb };
}

/** A link with no {@link FlatDevice} on it, so the far end sends only what a test says to. */
function bareLink(options: LoopbackOptions = {}) {
    const link = loopbackLink(options);
    return { link, usbDevice: new FakeUsbDevice(link) };
}

beforeAll(loadFlatDevice);

describe("browser support", () => {
    it("reports no WebUSB rather than pretending", () => {
        // Firefox and Safari take this path. The answer is the desktop app, not a retry.
        const watcher = new WebUsbWatcher({ usb: undefined });
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
        // On every visit after the first, `getDevices()` returns what the user already granted, no
        // gesture required. This is why `start()` never calls the chooser.
        const info: DeviceInfo = {
            firmwareRevision: "0.4.0+abc1234",
            hardwareRevision: "obc-lm20-r1",
            serialNumber: "0011223344556677",
        };
        const { usb, usbDevice, device } = rig({ deviceInfo: info });
        usb.permitted = [usbDevice];
        const watcher = new WebUsbWatcher({ usb });
        expect(await watcher.start()).toBe(true);
        expect(watcher.current.status).toBe("ready");

        // The two reads every connection makes. The identity strings come off EP0 before a record
        // moves; the store's identity comes out of the `LIST` every client issues first.
        expect(watcher.current.info).toEqual(info);
        expect(watcher.current.store).toEqual({ storeId: device.storeId, commitSequence: device.sequence });
        expect(usb.filtersSeen, "adopting must never open the chooser").toEqual([]);
        await watcher.close();
    });

    it("keeps the recovery client ready when LIST says the card is unformatted", async () => {
        const { usb, usbDevice } = rig({}, { formatted: false });
        usb.permitted = [usbDevice];
        const watcher = new WebUsbWatcher({ usb });
        expect(await watcher.start()).toBe(true);
        expect(watcher.current).toMatchObject({ status: "ready", store: null, error: null });

        const replacement = "2a7b16c4903145d8a6e2730fb94c5811";
        await expect(
            watcher.current.client?.format(null, { replacementStoreId: replacement }),
        ).resolves.toEqual({ storeId: replacement });
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
        usb.permitted = [new FakeUsbDevice(link, { vendorId: 0x1234, productId: 0x5678 })];
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

    it("releases the interface when the first exchange fails", async () => {
        // A device claimed but never listed still holds its interface, and USB grants it to one
        // claimant. Leaking it would leave the device unreachable until a physical re-plug. No device
        // runs on this link, so the `LIST` that follows the EP0 read times out.
        const { usbDevice } = bareLink();
        const usb = new FakeUsb();
        usb.permitted = [usbDevice];
        const watcher = new WebUsbWatcher({ usb, timeoutMs: 20 });
        expect(await watcher.start()).toBe(false);
        expect(watcher.current.status).toBe("error");
        expect(usbDevice.claimed, "the interface must be released").toBeNull();
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

describe("the USB-binding major, settled by matching", () => {
    // The descriptors state the major and the host refuses a device that contradicts it, before a
    // record moves. There is no version *read* on this link.

    it("accepts a device that states 5 in both places", () => {
        const layout = discoverLayout(configuration());
        expect(() =>
            checkUsbBindingMajor(
                { deviceVersionMajor: USB_BINDING_MAJOR } as UsbDeviceLike,
                layout,
                configuration(),
            ),
        ).not.toThrow();
    });

    it("tolerates a device that states neither", () => {
        // WebUSB exposes `deviceVersionMajor` everywhere but `interfaceProtocol` only on
        // `alternate`, and an older descriptor may carry neither. Saying nothing is not a
        // contradiction: it is left to fail on the first exchange, where the failure names a message.
        const config = configuration({ interfaceProtocol: null });
        expect(() => checkUsbBindingMajor({} as UsbDeviceLike, discoverLayout(config), config)).not.toThrow();
    });

    it("refuses a device whose `bcdDevice` contradicts 5", () => {
        const config = configuration();
        expect(() =>
            checkUsbBindingMajor({ deviceVersionMajor: 4 } as UsbDeviceLike, discoverLayout(config), config),
        ).toThrow(/uses USB binding v4; this page uses v5\. Update the device firmware, or reload the page/);
    });

    it("refuses a device whose `bInterfaceProtocol` contradicts 5", () => {
        // The other statement, checked independently: a device that got its `bcdDevice` right and
        // its interface descriptor wrong is still a device this page must not exchange records with.
        const config = configuration({ interfaceProtocol: 4 });
        expect(() =>
            checkUsbBindingMajor(
                { deviceVersionMajor: USB_BINDING_MAJOR } as UsbDeviceLike,
                discoverLayout(config),
                config,
            ),
        ).toThrow(/uses USB binding v4; this page uses v5/);
    });

    it("never claims the interface of a device that contradicts it", async () => {
        // The check sits between `selectConfiguration` and `claimInterface` on purpose: a mismatched
        // device is left entirely alone, so no other tab is locked out.
        const { usb, usbDevice } = rig({}, {}, { deviceVersionMajor: 3 });
        usb.permitted = [usbDevice];
        const watcher = new WebUsbWatcher({ usb });
        expect(await watcher.start()).toBe(false);
        expect(watcher.current.status).toBe("error");
        expect(watcher.current.error).toMatch(/USB binding v3/);
        expect(watcher.current.error).toMatch(/Update the device firmware, or reload the page/);
        expect(usbDevice.claimed).toBeNull();
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
        expect(watcher.current.store).toBeNull();
        await watcher.close();
    });

    it("fails a transfer in flight the moment the cable goes", async () => {
        // Unplugging must not leave a spinner. The pipes are failed from the event rather than left
        // for a pending `transferIn` to notice, because a pending one may never settle at all.
        const { usb, usbDevice, device } = rig({ packetSize: 64 }, { streamCeiling: streamCeilingFor(256) });
        usb.permitted = [usbDevice];
        const watcher = new WebUsbWatcher({ usb });
        await watcher.start();
        const client = watcher.current.client!;

        const bytes = Uint8Array.from({ length: 200_000 }, (_, i) => i & 0xff);
        const entry = device.seed({ kind: ObjectKind.Ride, displayName: "big", bytes });
        const started = Date.now();
        const download = client.get(
            { objectId: entry.objectId, revision: HEAD_REVISION },
            {
                onProgress: (done) => {
                    if (done > 512) usb.emit("disconnect", usbDevice);
                },
            },
        );
        const error = await download.catch((e: unknown) => e);
        expect(error).toBeInstanceOf(DeviceError);
        expect((error as DeviceError).code).toBe("link");
        expect(Date.now() - started).toBeLessThan(2_000);
        await watcher.close();
    });
});

describe("the endpoint layout", () => {
    it("takes the lowest IN/OUT pair as control and the next as stream", () => {
        // The channels are named after what the protocol puts on them, not after the endpoint type:
        // all four are bulk endpoints, and calling one pair "bulk" hid which one carries payloads.
        expect(discoverLayout(configuration())).toEqual({
            interfaceNumber: 2,
            control: { in: 1, out: 1, packetSize: 512 },
            stream: { in: 2, out: 2, packetSize: 512 },
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
                        endpoints: [{ endpointNumber: 1, direction: "in", type: "bulk", packetSize: 512 }],
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
        expect(usbDevice.claimed).toBe(2);
        await webusb.stream.reset();
        expect(usbDevice.halts).toEqual(["in2", "out2"]);
        await webusb.close();
        expect(usbDevice.claimed).toBeNull();
        await link.host.close();
    });

    it("sends a whole stream record, packet size be damned", async () => {
        // The ordinary stream frame — a 16-byte header plus an 8,192-byte payload — spans sixteen
        // 512-byte packets and a short one once the binding prefix is included. A rule that refused
        // any frame at or above the packet size would kill every upload.
        const { link, usbDevice } = bareLink();
        const webusb = await openWebUsbLink(usbDevice);
        const record = Uint8Array.from({ length: MAX_HOST_STREAM_RECORD }, (_, i) => (i * 13) & 0xff);
        await webusb.stream.write(record);

        // …and it really went out, across packet boundaries rather than truncated.
        const seen: Uint8Array[] = [];
        let got = 0;
        while (got < record.length) {
            const slice = await link.device.stream.read();
            seen.push(slice);
            got += slice.length;
        }
        expect(seen.length, "an 8,208-byte frame must span packets").toBeGreaterThan(1);
        const joined = new Uint8Array(got);
        let at = 0;
        for (const slice of seen) {
            joined.set(slice, at);
            at += slice.length;
        }
        expect(joined).toEqual(record);
        await webusb.close();
        await link.host.close();
    });

    it("reassembles a control record that arrives in pieces", async () => {
        // In the reading direction the length prefix is the only thing that says where a record ends,
        // so a reader has to accumulate. Eight-byte packets make the arithmetic visible: a 104-byte
        // padded record is thirteen transfers.
        const { link, usbDevice } = bareLink({ packetSize: 8 });
        const webusb = await openWebUsbLink(usbDevice);
        const channel = new RecordChannel(webusb.control, MAX_HOST_CONTROL_RECORD);
        const frame = Uint8Array.from({ length: 100 }, (_, i) => (i * 7 + 1) & 0xff);
        void link.device.control.write(frameRecord(frame));

        expect(await channel.next()).toEqual(frame);
        expect(usbDevice.reads, "13 packets carry a 104-byte record at 8 bytes each").toBe(13);
        expect(channel.buffered, "nothing may be left over after a record that fits exactly").toBe(0);
        await webusb.close();
        await link.host.close();
    });

    it("pads odd-sized frames so the following record remains word aligned", async () => {
        const frame = Uint8Array.from({ length: 5 }, (_, i) => i + 1);
        const record = frameRecord(frame);
        expect(record.length).toBe(12);
        expect([...record.subarray(0, 4)]).toEqual([5, 0, 0, 0]);
        expect([...record.subarray(9)]).toEqual([0, 0, 0]);

        const { link, usbDevice } = bareLink({ packetSize: 8 });
        const webusb = await openWebUsbLink(usbDevice);
        const channel = new RecordChannel(webusb.control, MAX_HOST_CONTROL_RECORD);
        void link.device.control.write(record);
        expect(await channel.next()).toEqual(frame);
        expect(channel.buffered).toBe(0);
        await webusb.close();
        await link.host.close();
    });

    it("rejects non-zero record padding", async () => {
        const record = frameRecord(new Uint8Array([0xaa]));
        record[record.length - 1] = 1;
        const { link, usbDevice } = bareLink();
        const webusb = await openWebUsbLink(usbDevice);
        const channel = new RecordChannel(webusb.control, MAX_HOST_CONTROL_RECORD);
        void link.device.control.write(record);
        await expect(channel.next()).rejects.toThrow(/non-zero USB record padding/);
        await webusb.close();
        await link.host.close();
    });

    /**
     * A cancelled read, at the pipe seam.
     *
     * The fake's `transferIn` parks a reader on the loopback channel, and the channel hands each new
     * slice to the *longest-waiting* reader — which is how a bulk endpoint serves queued transfers,
     * and the only reason these three tests say anything about hardware.
     */
    describe("after a cancelled read", () => {
        /** Park a stream read, cancel it, and reset — the client's failure path, one layer down. */
        async function cancelAParkedRead(webusb: Awaited<ReturnType<typeof openWebUsbLink>>) {
            const controller = new AbortController();
            const parked = webusb.stream.read(controller.signal);
            controller.abort();
            await expect(parked).rejects.toMatchObject({ code: "aborted" });
            await webusb.stream.reset();
        }

        it("hands the abandoned transfer to the next reader", async () => {
            const { link, usbDevice } = bareLink();
            const webusb = await openWebUsbLink(usbDevice);
            await cancelAParkedRead(webusb);

            // The next bytes on the endpoint belong to the next transfer, because the cancel has the
            // device drop the old one. Submitting a *second* `transferIn` would queue it behind the
            // abandoned one, and the abandoned one would take this packet and bin it.
            void link.device.stream.write(new Uint8Array([1, 2, 3]));
            expect(await webusb.stream.read()).toEqual(new Uint8Array([1, 2, 3]));
            await webusb.close();
            await link.host.close();
        });

        it("does not keep a transfer that already took the aborted transfer's last packet", async () => {
            // The device only stops when the cancel reaches it, so the transfer the caller walked
            // away from may complete with one last packet of the transfer being abandoned. Keeping
            // that would prepend a stale packet to the next one. `reset` runs before the transfer
            // slot is released, so anything settled by then is stale by construction.
            const { link, usbDevice } = bareLink();
            const webusb = await openWebUsbLink(usbDevice);
            const controller = new AbortController();
            const parked = webusb.stream.read(controller.signal);
            controller.abort();
            await expect(parked).rejects.toMatchObject({ code: "aborted" });
            void link.device.stream.write(new Uint8Array([0xde, 0xad]));
            // A macrotask boundary, so every microtask between the write and the transfer marking
            // itself settled has run.
            await new Promise((resolve) => setTimeout(resolve, 0));
            await webusb.stream.reset();

            void link.device.stream.write(new Uint8Array([1, 2, 3]));
            expect(await webusb.stream.read()).toEqual(new Uint8Array([1, 2, 3]));
            await webusb.close();
            await link.host.close();
        });

        it("leaves the busy half's halt alone", async () => {
            // `clearHalt` resets the endpoint's data toggle, which must not happen under a live
            // transfer — and the IN half cannot be halted while one is outstanding anyway.
            const { link, usbDevice } = bareLink();
            const webusb = await openWebUsbLink(usbDevice);
            await cancelAParkedRead(webusb);
            expect(usbDevice.halts).toEqual(["out2"]);
            await webusb.close();
            await link.host.close();
        });

        it("does not cost the next object its first packet", async () => {
            // One whole `GET`, on a link a cancel has been through. Discarding the abandoned
            // transfer would have eaten this ride's first packet.
            const { link, usbDevice, device } = rig({ packetSize: 64 }, { streamCeiling: streamCeilingFor(256) });
            const bytes = Uint8Array.from({ length: 4_096 }, (_, i) => (i * 7) & 0xff);
            const entry = device.seed({ kind: ObjectKind.Ride, displayName: "after the cancel", bytes });
            const webusb = await openWebUsbLink(usbDevice);
            const client = new FlatStoreClient(webusb);
            await cancelAParkedRead(webusb);

            const got = await client.get({ objectId: entry.objectId, revision: HEAD_REVISION });
            expect(got.bytes).toEqual(bytes);
            await client.close();
            await link.host.close();
        });
    });

    it("absorbs a zero-length packet instead of handing back an empty read", async () => {
        // A ZLP is a USB-level marker, not data, and returning it looks like a spurious wakeup.
        const { link, usbDevice } = bareLink();
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
        void link.device.stream.write(new Uint8Array([1, 2, 3]));
        expect(await webusb.stream.read()).toEqual(new Uint8Array([1, 2, 3]));
        await webusb.close();
        await link.host.close();
    });
});

describe("the EP0 device-info read", () => {
    it("asks §5.2.1's exact question, on the interface it claimed", async () => {
        // Recipient **interface** rather than device, so the request cannot collide with the
        // device-level MS OS 2.0 descriptor request the same device answers for Windows.
        const { link, usbDevice } = rig();
        const webusb = await openWebUsbLink(usbDevice);
        const payload = await webusb.vendorIn!(GET_DEVICE_INFO, 0, DEVICE_INFO_MAX);

        expect(usbDevice.setups).toEqual([
            {
            // `index` is the **claimed** interface number: the rig enumerates 2, so a hard-coded
            // zero would fail here rather than pass by coincidence.
                setup: { requestType: "vendor", recipient: "interface", request: 0x20, value: 0, index: 2 },
                length: DEVICE_INFO_MAX,
            },
        ]);
        // A short transfer is what to expect — the three strings are nowhere near 192 bytes — so a
        // host that assumed it got `length` back would fail on this device.
        expect(payload.length).toBeLessThan(DEVICE_INFO_MAX);
        expect(decodeDeviceInfo(payload).hardwareRevision).toBe("obc-lm20-r1");
        await webusb.close();
        await link.host.close();
    });

    it("treats a stalled control request as a transport failure", async () => {
        const { link, usbDevice } = rig();
        usbDevice.controlAnswer = async () => ({ status: "stall" });
        const webusb = await openWebUsbLink(usbDevice);
        await expect(webusb.vendorIn!(GET_DEVICE_INFO, 0, DEVICE_INFO_MAX)).rejects.toMatchObject({
            name: "PipeError",
            code: "device-error",
        });
        await webusb.close();
        await link.host.close();
    });

    it("treats an empty answer as a stall in all but name", async () => {
        // The device returns its three strings, so "ok with nothing" is a device that declined the
        // request without saying so, and decoding zero bytes into a firmware version invents one.
        const { link, usbDevice } = rig();
        usbDevice.controlAnswer = async () => ({ status: "ok", data: new DataView(new ArrayBuffer(0)) });
        const webusb = await openWebUsbLink(usbDevice);
        await expect(webusb.vendorIn!(GET_DEVICE_INFO, 0, DEVICE_INFO_MAX)).rejects.toMatchObject({
            name: "PipeError",
            code: "device-error",
        });
        await webusb.close();
        await link.host.close();
    });
});
