/**
 * The cable, under Node.
 *
 * `usb`'s WebUSB object answers the same calls `navigator.usb` does, so the shipping
 * `openWebUsbLink` opens the device here exactly as it does in the browser: the endpoint layout,
 * the USB-binding check, the two byte pipes and the EP0 read are all the ones the product uses.
 * This file is an adapter and nothing more.
 *
 * The one real difference is the timeout. WebUSB submits a transfer and waits for the device
 * indefinitely, which is what the client's own deadlines are written against; `usb` defaults every
 * transfer to one second, and a one-second read would abandon the control channel between two of
 * the device's answers. Every call is therefore re-issued with a bound far above any phase deadline,
 * so the run's own clock is the only one that can fire.
 */

import { WebUSB } from "usb";

import { FlatStoreClient } from "../../src/lib/usb/client";
import type { DeviceSession } from "../../test-support/device-smoke/smoke";
import {
    OBC_USB_FILTERS,
    openWebUsbLink,
    type UsbControlInResult,
    type UsbControlSetup,
    type UsbDeviceLike,
    type UsbInResult,
    type UsbOutResult,
} from "../../src/lib/usb/webusb";

/**
 * The per-transfer timeout `usb` adds to each W3C call. It is not in `@types/w3c-web-usb`, because
 * the browser has nowhere to put it, so the shape is stated here and applied by one cast.
 */
interface TimedTransfers {
    transferIn(endpointNumber: number, length: number, timeout: number): Promise<UsbInResult>;
    transferOut(endpointNumber: number, data: Uint8Array, timeout: number): Promise<UsbOutResult>;
    controlTransferIn(setup: UsbControlSetup, length: number, timeout: number): Promise<UsbControlInResult>;
}

type NodeUsbDevice = Awaited<ReturnType<WebUSB["getDevices"]>>[number] & TimedTransfers;

/** Longer than any phase deadline, so a transfer never expires before the run gives up. */
const TRANSFER_TIMEOUT_MS = 3_600_000;

/** How often enumeration is looked at again while waiting for a device to come back. */
const ENUMERATION_POLL_MS = 250;

/** `usb`'s own WebUSB, allowed every device because a Node process has no chooser to ask. */
const webusb = new WebUSB({ allowAllDevices: true });

/** Every attached device that matches the product's VID/PID. */
async function candidates(): Promise<NodeUsbDevice[]> {
    const devices = (await webusb.getDevices()) as NodeUsbDevice[];
    return devices.filter((device) =>
        OBC_USB_FILTERS.some(
            (filter) =>
                device.vendorId === filter.vendorId &&
                (filter.productId === undefined || device.productId === filter.productId),
        ),
    );
}

/**
 * Wait for the device to be enumerated, then open a session on it.
 *
 * Enumeration is polled rather than awaited. A reset takes the cable down and the host brings it
 * back on its own schedule; there is no completion the process can wait on, and the hot-plug event
 * fires whether or not a listener was attached in time. The bound is the caller's deadline.
 */
export async function connect(signal: AbortSignal, serial?: string): Promise<DeviceSession> {
    for (;;) {
        signal.throwIfAborted();
        const found = (await candidates()).filter((device) => !serial || device.serialNumber === serial);
        if (found.length > 1) {
            const serials = found.map((device) => device.serialNumber ?? "(no serial)").join(", ");
            throw new Error(`${found.length} devices are attached (${serials}). Name one with --serial.`);
        }
        if (found.length === 1) {
            const link = await openWebUsbLink(patient(found[0]));
            return { client: new FlatStoreClient(link), release: () => link.close() };
        }
        await new Promise((resolve) => setTimeout(resolve, ENUMERATION_POLL_MS));
    }
}

/** The same device, with every transfer given the run's clock instead of the library's one second. */
function patient(device: NodeUsbDevice): UsbDeviceLike {
    return {
        get vendorId() {
            return device.vendorId;
        },
        get productId() {
            return device.productId;
        },
        get deviceVersionMajor() {
            return device.deviceVersionMajor;
        },
        get serialNumber() {
            return device.serialNumber ?? undefined;
        },
        get productName() {
            return device.productName ?? undefined;
        },
        get opened() {
            return device.opened;
        },
        get configuration() {
            // `usb` throws rather than answering `null` on a device that has no configuration
            // selected yet, which is the state `openWebUsbLink` selects one from.
            try {
                return device.configuration ?? null;
            } catch {
                return null;
            }
        },
        open: () => device.open(),
        close: () => device.close(),
        selectConfiguration: (value) => device.selectConfiguration(value),
        claimInterface: (number) => device.claimInterface(number),
        releaseInterface: (number) => device.releaseInterface(number),
        transferIn: (endpoint, length) => device.transferIn(endpoint, length, TRANSFER_TIMEOUT_MS),
        transferOut: (endpoint, data) => device.transferOut(endpoint, data, TRANSFER_TIMEOUT_MS),
        controlTransferIn: (setup, length) => device.controlTransferIn(setup, length, TRANSFER_TIMEOUT_MS),
        clearHalt: (direction, endpoint) => device.clearHalt(direction, endpoint),
    };
}
