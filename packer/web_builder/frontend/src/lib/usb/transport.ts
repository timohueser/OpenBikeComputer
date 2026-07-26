/**
 * The one thing USB adds to the interface spec: a frame that says which of BLE's control
 * characteristics a control message belongs to.
 *
 * ## Why anything is needed at all
 *
 * BLE splits the protocol across two planes. Bulk bytes ride an L2CAP CoC — reliable, ordered, and
 * deliberately unframed — and a USB bulk endpoint is the same thing, so the object stream needs no
 * translation whatsoever. But the *control* plane is GATT: seven separately-addressed
 * characteristics, where "which characteristic" is carried by the transport rather than by any
 * byte of ours. USB has one endpoint pair, so that routing has to become a byte.
 *
 * One byte is what it becomes. Every control frame is `selector u8 · payload`, and **the payload is
 * the exact bytes the corresponding GATT characteristic carries** — the same bytes
 * `protocol-vectors/` pins and the firmware and iOS already encode. Nothing about the object model,
 * the descriptors, the status envelope or the CRC changes. USB is a second transport, not a second
 * protocol.
 *
 * ## Status: provisional, and owned jointly with #889
 *
 * #889 owns the device side — the embassy-nrf USBHS driver question, the VID/PID allocation, and
 * the endpoint layout — and it has not landed. This envelope is therefore the host's *proposal*,
 * and if #889 ratifies a different one, here is honestly what moves.
 *
 * **The wire encoding is confined to this file**: the selector values, {@link encodeFrame} /
 * {@link decodeFrame}, and the device-info string packing. Nothing else builds or parses a frame.
 *
 * **The *assumption* that control messages are routed by a leading selector is not** — it is shared
 * with `client.ts`, whose `dispatch` switches on `frame.selector`, and `loopback.ts`, which switches
 * on it and encodes replies. A scheme that routed some other way (separate endpoints per
 * characteristic, a CDC line protocol) would touch those two as well. Small, mechanical edits, but
 * three files rather than one — worth knowing before you start.
 *
 * **What does not move at all, under any envelope**: the object model, the transfer descriptors, the
 * status envelope, the commands, the object layouts, the CRC-32, and every `protocol-vectors/`
 * fixture. Those live in `protocol.ts` / `objects.ts` and are the reason USB is a second transport
 * rather than a second protocol. That is the claim worth protecting; the rest is plumbing.
 */

import { DecodeError } from "./protocol";

/** Host → device selectors. Each names the GATT characteristic the payload would have been
 *  written to (§3.3). */
export const HostFrame = {
    /** `command` — a §4.4 imperative. */
    Command: 1,
    /** `transferControl` — the §4.2 12-byte descriptor. */
    TransferControl: 2,
    /** `config` write — the §7.3 blob. */
    ConfigWrite: 3,
    /** `protocolVersion` read (§1). No payload. */
    IdentityRead: 4,
    /** The Device Information Service strings (§3.1). No payload. */
    DeviceInfoRead: 5,
    /** `config` read (§7.3). No payload. */
    ConfigRead: 6,
} as const;
export type HostFrame = (typeof HostFrame)[keyof typeof HostFrame];

/** Device → host selectors. */
export const DeviceFrame = {
    /** `status` — the §4.3 envelope verbatim, discriminator byte included. This is the sole
     *  unsolicited channel, exactly as on BLE: one ordering domain for every device → host edge. */
    Status: 1,
    /** The answer to {@link HostFrame.IdentityRead}: the §1 bytes, 6 with a store, 2 without. */
    Identity: 2,
    /** The answer to {@link HostFrame.DeviceInfoRead}: see {@link encodeDeviceInfo}. */
    DeviceInfo: 3,
    /** The answer to {@link HostFrame.ConfigRead}: the §7.3 blob. */
    Config: 4,
} as const;
export type DeviceFrame = (typeof DeviceFrame)[keyof typeof DeviceFrame];

/** A decoded control frame: its selector and the payload after it. */
export interface ControlFrame {
    selector: number;
    payload: Uint8Array;
}

/** Prefix `payload` with `selector`. */
export function encodeFrame(selector: number, payload?: Uint8Array): Uint8Array {
    const out = new Uint8Array(1 + (payload?.length ?? 0));
    out[0] = selector;
    if (payload) out.set(payload, 1);
    return out;
}

export function decodeFrame(data: Uint8Array): ControlFrame {
    if (data.length < 1) throw new DecodeError("truncated", "an empty control frame arrived.");
    return { selector: data[0], payload: data.subarray(1) };
}

/**
 * The Device Information Service strings (§3.1), which have no binary layout of their own because
 * on BLE they are three separate characteristics: `len u8 · UTF-8`, three times, in the order
 * firmware · hardware · serial.
 *
 * The firmware revision is the load-bearing one — it is what "an update is available" compares
 * against, and the spec is explicit that the *running* image's version lives here and nowhere else
 * (never duplicated into the Config object, where the two could disagree).
 */
export interface DeviceInfo {
    /** e.g. `0.4.0+abc1234` — the running image, after a confirmed DFU the new one. */
    firmwareRevision: string;
    /** e.g. `obc-lm20-r1`. */
    hardwareRevision: string;
    /** 16 uppercase hex digits — the nRF `FICR.DEVICEID`. */
    serialNumber: string;
}

export function encodeDeviceInfo(info: DeviceInfo): Uint8Array {
    const parts = [info.firmwareRevision, info.hardwareRevision, info.serialNumber].map((s) =>
        new TextEncoder().encode(s),
    );
    const out = new Uint8Array(parts.reduce((n, p) => n + 1 + p.length, 0));
    let at = 0;
    for (const p of parts) {
        if (p.length > 0xff) throw new RangeError(`a device-info string is ${p.length} bytes, the cap is 255.`);
        out[at++] = p.length;
        out.set(p, at);
        at += p.length;
    }
    return out;
}

export function decodeDeviceInfo(data: Uint8Array): DeviceInfo {
    const decoder = new TextDecoder();
    const strings: string[] = [];
    let at = 0;
    for (let i = 0; i < 3; i++) {
        if (at >= data.length) throw new DecodeError("truncated", `device info carries ${i} of 3 strings.`);
        const len = data[at++];
        if (at + len > data.length) {
            throw new DecodeError("truncated", `device-info string ${i} claims ${len} bytes past the frame's end.`);
        }
        strings.push(decoder.decode(data.subarray(at, at + len)));
        at += len;
    }
    return { firmwareRevision: strings[0], hardwareRevision: strings[1], serialNumber: strings[2] };
}
