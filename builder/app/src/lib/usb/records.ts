/**
 * §5.2's USB binding v5 framing: the aligned records that carry protocol-v4 frames over a byte pipe.
 *
 * [`FLAT_Store_Protocol.md`](../../../../../specs/FLAT_Store_Protocol.md) §5.2 gives USB two bulk
 * endpoint pairs and one rule for both: *each record is `record_length u32`, exactly that many frame
 * bytes, then zero padding to a four-byte boundary*. Everything interesting about this file follows
 * from the sentence after it —
 * **packet boundaries carry no protocol meaning; a record may span packets** — because that is
 * precisely the property a naive reader gets wrong.
 *
 * The v1 envelope this replaces assumed one USB transfer was one frame, so a control message that
 * reached the endpoint's max packet size was a protocol error the host refused to send. Under v4 a
 * 8,208-byte device→host record spans seventeen 512-byte packets on a high-speed endpoint, and there is no
 * length in a packet to tell the reader which one ends the record. So {@link RecordChannel} keeps a
 * buffer and re-reads its own prefix, and {@link frameRecord} is the only thing that ever writes
 * one.
 *
 * ## Ceilings (§5.2, "Record ceilings are a constant of this binding")
 *
 * Fixed here rather than negotiated, because §3 has no capability discovery and USB offers nothing
 * to derive a ceiling from. A device→host record above {@link MAX_DEVICE_RECORD} is a framing error
 * the reader refuses rather than a large frame it tries to assemble: believing an absurd length
 * would park the read loop forever on bytes that are never coming.
 *
 * ## What this file is not
 *
 * It does not know what a frame means. A control record and a stream record are the same shape here
 * and differ only in the ceiling the channel was built with, which is why one reader serves both.
 * Interpretation is `protocol.ts`'s, and correlation is `client.ts`'s.
 */

import { PipeError, throwIfAborted, type BytePipe } from "./pipe";

/** The USB-binding version advertised before a record is exchanged (§5.2). */
export const USB_BINDING_MAJOR = 5;

/** The four-byte length prefix every record carries (§5.2). */
export const RECORD_PREFIX_LEN = 4;

/** The word alignment guaranteed for every prefix, frame, and following record. */
export const RECORD_ALIGNMENT = 4;

/** Frame bytes plus the binding-level zero padding that follows them. */
export function paddedRecordLen(frameLen: number): number {
    return Math.ceil(frameLen / RECORD_ALIGNMENT) * RECORD_ALIGNMENT;
}

/**
 * Device → host, either channel: §3.8's 16-byte stream frame plus 8,192 payload bytes.
 */
export const MAX_DEVICE_RECORD = 8208;

/** Host → device, stream channel: the same number, so a client frames both directions alike. */
export const MAX_HOST_STREAM_RECORD = 8208;

/**
 * Host → device, control channel. §3's largest request is the 100-byte `PUT`; the device sizes this
 * buffer to the protocol rather than to the ceiling, and a longer control record is `invalidFrame`
 * with detail `length`.
 */
export const MAX_HOST_CONTROL_RECORD = 256;

/**
 * The largest payload a stream record may carry (§5.2's closing rule).
 *
 * A client MUST NOT exceed it, and §3.8 already makes a length above the link's ceiling terminate
 * the transfer — so a client that got this wrong would not be sending slightly-too-large records,
 * it would be killing every upload. Full records of exactly this many bytes are what the device
 * writes to the card in one go, which is why the upload loop sends them rather than something
 * rounder.
 */
export const MAX_STREAM_PAYLOAD = MAX_HOST_STREAM_RECORD - 16;

/** A record whose length prefix cannot be honoured — malformed, or above the channel's ceiling. */
export class RecordError extends Error {
    constructor(message: string) {
        super(message);
        this.name = "RecordError";
    }
}

/** Prefix and pad `frame` according to USB binding v5. The whole of §5.2's host-side framing. */
export function frameRecord(frame: Uint8Array): Uint8Array {
    if (frame.length === 0 || frame.length > 0xffffffff) {
        throw new RecordError(`a record carries 1..=4294967295 frame bytes, this one has ${frame.length}.`);
    }
    const out = new Uint8Array(RECORD_PREFIX_LEN + paddedRecordLen(frame.length));
    out[0] = frame.length & 0xff;
    out[1] = (frame.length >>> 8) & 0xff;
    out[2] = (frame.length >>> 16) & 0xff;
    out[3] = (frame.length >>> 24) & 0xff;
    out.set(frame, RECORD_PREFIX_LEN);
    return out;
}

/**
 * One record channel: a length-prefixed writer and a reader that reassembles across packets.
 *
 * Built per direction pair rather than per direction, because the reader's leftover buffer and the
 * writer's ceiling belong to the same endpoint pair and separating them only invites two objects
 * that disagree about which pipe they are on.
 */
export class RecordChannel {
    private pending: Uint8Array = new Uint8Array(0);

    /**
     * Both ceilings measure the **frame**, not the frame plus its prefix.
     *
     * §5.2's table is stated in the frame's own terms — "§3.8's 16-byte stream frame plus 8,192
     * payload bytes" is 8,208 — and the prefix/padding are the binding's own overhead on top. A
     * ceiling that counted them would refuse the largest legal record by exactly four bytes, which
     * is the one number this protocol is built around.
     */
    constructor(
        private readonly pipe: BytePipe,
        /** Ceiling on a frame this side **sends** (§5.2's host → device row). */
        private readonly sendCeiling: number,
        /** Ceiling on a frame this side **accepts** (§5.2's device → host row). */
        private readonly receiveCeiling: number = MAX_DEVICE_RECORD,
    ) {}

    /** Bytes read off the pipe but not yet consumed by a record. Diagnostics and tests only. */
    get buffered(): number {
        return this.pending.length;
    }

    /** Send one frame as one record. Resolves once the transport has taken it. */
    async send(frame: Uint8Array, signal?: AbortSignal): Promise<void> {
        if (frame.length > this.sendCeiling) {
            throw new RecordError(
                `a ${frame.length}-byte frame is above this channel's ${this.sendCeiling}-byte ceiling.`,
            );
        }
        await this.pipe.write(frameRecord(frame), signal);
    }

    /**
     * The next whole record's frame bytes.
     *
     * Reads until the prefix and then the body are complete, so a record split across any number of
     * packets — or two records coalesced into one read — both come out right. The returned array is
     * a copy, never a view into the buffer the next call will overwrite.
     */
    async next(signal?: AbortSignal): Promise<Uint8Array> {
        for (;;) {
            if (this.pending.length >= RECORD_PREFIX_LEN) {
                const length =
                    (this.pending[0] |
                        (this.pending[1] << 8) |
                        (this.pending[2] << 16) |
                        (this.pending[3] << 24)) >>>
                    0;
                // §5.2: "A zero, out-of-range, truncated or overrun record length is `invalidFrame`
                // and resets that record stream." A host that kept reading past one would be
                // assembling frames out of the middle of somebody else's record.
                if (length === 0) throw new RecordError("the device sent a zero-length record.");
                if (length > this.receiveCeiling) {
                    throw new RecordError(
                        `the device announced a ${length}-byte record, above the ` +
                            `${this.receiveCeiling}-byte ceiling of §5.2.`,
                    );
                }
                const padded = paddedRecordLen(length);
                if (this.pending.length >= RECORD_PREFIX_LEN + padded) {
                    const frame = this.pending.slice(RECORD_PREFIX_LEN, RECORD_PREFIX_LEN + length);
                    const padding = this.pending.subarray(RECORD_PREFIX_LEN + length, RECORD_PREFIX_LEN + padded);
                    if (padding.some((byte) => byte !== 0)) {
                        throw new RecordError("the device sent non-zero USB record padding.");
                    }
                    this.pending = this.pending.slice(RECORD_PREFIX_LEN + padded);
                    return frame;
                }
            }
            throwIfAborted(signal, "the record read");
            this.pending = concat(this.pending, await this.pipe.read(signal));
        }
    }

    /**
     * Drop whatever a partial record left behind.
     *
     * A channel abandoned mid-record is at an unknown offset, so the bytes held here belong to a
     * frame nobody will finish. They are dropped **with** the pipe's own reset rather than instead
     * of it: this clears what the host already pulled off the endpoint, `BytePipe.reset` deals with
     * what is still on it.
     */
    async reset(): Promise<void> {
        this.pending = new Uint8Array(0);
        await this.pipe.reset();
    }
}

// --- §5.2.1's EP0 payload ---------------------------------------------------------
//
// Not a record and not a §3 frame, and here anyway: both halves of this file are the *USB binding*
// rather than the protocol. §5.2 gives all of the bulk pairs to §3, so USB's equivalent of BLE's
// separately-addressed control characteristics is EP0, where every USB device's identity already
// lives. Putting this codec in `protocol.ts` would put a transport fact inside the file whose whole
// claim is that its bytes are identical on both links.

/** The vendor request number §5.2.1 registers: `GET_DEVICE_INFO`. */
export const GET_DEVICE_INFO = 0x20;

/** The payload ceiling §5.2.1 states: three strings of at most 48 bytes, each with a length byte. */
export const DEVICE_INFO_MAX = 192;

/**
 * The three strings §5.2.1's payload carries, in its order.
 *
 * The firmware revision is the load-bearing one — it is what "an update is available" compares
 * against, and the running image's version lives there and nowhere else.
 */
export interface DeviceInfo {
    /** e.g. `0.4.0+abc1234` — the running image, after a confirmed DFU the new one. */
    firmwareRevision: string;
    /** e.g. `obc-lm20-r1`. */
    hardwareRevision: string;
    /** 16 uppercase hex digits — the nRF `FICR.DEVICEID`. */
    serialNumber: string;
}

/** `len u8 · UTF-8`, three times. */
export function encodeDeviceInfo(info: DeviceInfo): Uint8Array {
    const parts = [info.firmwareRevision, info.hardwareRevision, info.serialNumber].map((s) =>
        new TextEncoder().encode(s),
    );
    const out = new Uint8Array(parts.reduce((n, p) => n + 1 + p.length, 0));
    let at = 0;
    for (const part of parts) {
        if (part.length > 0xff) throw new RecordError(`a device-info string is ${part.length} bytes, the cap is 255.`);
        out[at++] = part.length;
        out.set(part, at);
        at += part.length;
    }
    return out;
}

/** Read §5.2.1's payload. A short transfer that stops inside a string is a malformed answer. */
export function decodeDeviceInfo(data: Uint8Array): DeviceInfo {
    const decoder = new TextDecoder();
    const strings: string[] = [];
    let at = 0;
    for (let i = 0; i < 3; i++) {
        if (at >= data.length) throw new RecordError(`device info carries ${i} of 3 strings.`);
        const length = data[at++];
        if (at + length > data.length) {
            throw new RecordError(`device-info string ${i} claims ${length} bytes past the payload's end.`);
        }
        strings.push(decoder.decode(data.subarray(at, at + length)));
        at += length;
    }
    return { firmwareRevision: strings[0], hardwareRevision: strings[1], serialNumber: strings[2] };
}

function concat(head: Uint8Array, tail: Uint8Array): Uint8Array {
    if (head.length === 0) return tail;
    const out = new Uint8Array(head.length + tail.length);
    out.set(head, 0);
    out.set(tail, head.length);
    return out;
}

/** Re-exported so a caller catching record framing does not have to import two modules. */
export { PipeError };
