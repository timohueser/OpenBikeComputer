/**
 * The stream frame, fault body and teardown vocabulary of Device_Object_Protocol_v3.md §13.
 *
 * §13's flag table is exhaustive per direction and "every other combination is `invalidFrame`",
 * which is why the flag check here is a whitelist rather than a mask test: a data direction carries
 * zero flags, a status direction carries the fault bit alone or the fault and terminal bits
 * together, and terminal-without-fault is reserved because a stream has no successful terminal
 * frame — success is FinishUpload or FinishDownload on the control link.
 */

import { Cursor, Writer } from "./bytes";
import { CATEGORY, CATEGORY_NAME, detailIsRegistered, detailName, reject, type CategoryName } from "./result";

export const STREAM_HEADER_BYTES = 16;
export const MIN_STREAM_FRAME_BYTES = 64;
export const MAX_STREAM_FRAME_BYTES = 4096;
export const FAULT_BODY_BYTES = 24;

export const STREAM_DIRECTION = { upload: 1, download: 2, status: 3 } as const;
export const STREAM_FLAG = { fault: 1 << 0, terminal: 1 << 1 } as const;
const STREAM_FLAG_MASK = 0x03;

export const FAULT_DISPOSITION = {
    resumeWithNewSession: 0,
    operationDurablyAborted: 1,
    streamTransportClosed: 2,
} as const;

const U64_MAX = (1n << 64n) - 1n;

/**
 * §13's transport set: "exactly these ten categories and no others". It is a closed list rather
 * than a rule of thumb about what feels transport-shaped, and the two exclusions are the reason it
 * had to be written down.
 *
 * `resourceLimit` is out because "every bounded resource a stream could exhaust is reserved at
 * admission, so an attached session has no resource-limit condition to report" — a fault can only
 * be raised by a session that already holds its slots. `semanticValidation` is out because the
 * compact body has no namespace field to scope its detail, so a domain outcome needs the correlated
 * control response instead.
 */
const TRANSPORT_CATEGORIES: readonly number[] = [
    CATEGORY.invalidFrame,
    CATEGORY.invalidDescriptor,
    CATEGORY.invalidOffset,
    CATEGORY.invalidSession,
    CATEGORY.checksumFailure,
    CATEGORY.mediaUnavailable,
    CATEGORY.mediaIo,
    CATEGORY.cancelled,
    CATEGORY.linkLost,
    CATEGORY.internal,
];

export interface StreamFault {
    readonly categoryValue: number;
    readonly category: CategoryName;
    readonly detailValue: number;
    readonly detail: string;
    readonly expectedNextOffset: bigint;
    readonly durableNextOffset: bigint;
    readonly disposition: number;
}

export interface StreamFrame {
    readonly sessionId: number;
    readonly offset: bigint;
    readonly direction: number;
    readonly flags: number;
    readonly payload: Uint8Array;
    /** Present exactly when the frame is a status fault. */
    readonly fault?: StreamFault;
}

export interface StreamDecodeOptions {
    /** The effective stream limit, `min(negotiated stream maximum, CoC SDU)` for BLE (§14.0). */
    readonly maximumFrameBytes?: number;
}

export function decodeStreamFrame(bytes: Uint8Array, options: StreamDecodeOptions = {}): StreamFrame {
    if (bytes.length < STREAM_HEADER_BYTES) {
        reject("invalidFrame", "recordLength", "a stream record carries a complete 16-byte header");
    }
    const limit = options.maximumFrameBytes ?? MAX_STREAM_FRAME_BYTES;
    if (bytes.length > limit) {
        reject("invalidFrame", "frameBounds", `a stream frame of ${bytes.length} bytes exceeds the ${limit}-byte limit`);
    }

    const cursor = new Cursor(bytes);
    const sessionId = cursor.u32();
    const offset = cursor.u64();
    const payloadLength = cursor.u16();
    const direction = cursor.u8();
    const flags = cursor.u8();

    if (sessionId === 0) reject("invalidDescriptor", "unknownEnum", "every stream frame carries a nonzero SessionId");
    if (direction !== STREAM_DIRECTION.upload && direction !== STREAM_DIRECTION.download && direction !== STREAM_DIRECTION.status) {
        reject("invalidDescriptor", "unknownEnum", `stream direction ${direction} is not registered`);
    }
    if ((flags & ~STREAM_FLAG_MASK) !== 0) {
        reject("invalidFrame", "malformedHeader", "stream flags above bit 1 are reserved");
    }
    if (direction === STREAM_DIRECTION.status) {
        if (flags !== STREAM_FLAG.fault && flags !== (STREAM_FLAG.fault | STREAM_FLAG.terminal)) {
            reject("invalidFrame", "malformedHeader", "a status frame carries the fault bit, optionally with terminal");
        }
    } else if (flags !== 0) {
        reject("invalidFrame", "malformedHeader", "any nonzero flag on a data direction is rejected");
    }

    if (cursor.remaining !== payloadLength) {
        reject("invalidFrame", "payloadLength", "the payload length disagrees with the record length");
    }
    if (direction !== STREAM_DIRECTION.status && payloadLength === 0) {
        reject("invalidFrame", "payloadLength", "data directions have a nonempty payload");
    }
    if (offset + BigInt(payloadLength) > U64_MAX) {
        reject("invalidFrame", "payloadLength", "offset + length overflows the 64-bit offset space");
    }
    if (direction === STREAM_DIRECTION.status && offset !== 0n) {
        reject("invalidDescriptor", "reservedBits", "a status frame has offset zero");
    }

    const payload = cursor.take(payloadLength);
    if (direction !== STREAM_DIRECTION.status) return { sessionId, offset, direction, flags, payload };
    return { sessionId, offset, direction, flags, payload, fault: decodeStreamFault(payload, flags) };
}

function decodeStreamFault(body: Uint8Array, flags: number): StreamFault {
    if (body.length !== FAULT_BODY_BYTES) {
        reject("invalidFrame", "payloadLength", `a fault status contains exactly ${FAULT_BODY_BYTES} bytes`);
    }
    const cursor = new Cursor(body);
    const categoryValue = cursor.u16();
    const detailValue = cursor.u16();
    const expectedNextOffset = cursor.u64();
    const durableNextOffset = cursor.u64();
    const disposition = cursor.u8();
    cursor.zeros(3, "fault body reserved bytes");

    if (!TRANSPORT_CATEGORIES.includes(categoryValue)) {
        reject(
            "invalidDescriptor",
            "unknownEnum",
            `category ${categoryValue} is not a transport category a stream fault may carry`,
        );
    }
    const category = CATEGORY_NAME.get(categoryValue) as CategoryName;
    if (!detailIsRegistered(category, detailValue)) {
        reject("invalidDescriptor", "unknownEnum", `detail ${detailValue} is not registered for ${category}`);
    }
    if (disposition > FAULT_DISPOSITION.streamTransportClosed) {
        reject("invalidDescriptor", "unknownEnum", `fault disposition ${disposition} is not registered`);
    }
    const terminal = (flags & STREAM_FLAG.terminal) !== 0;
    if (terminal !== (disposition !== FAULT_DISPOSITION.resumeWithNewSession)) {
        reject(
            "invalidDescriptor",
            "invalidCombination",
            "disposition 0 is the nonterminal fault; dispositions 1 and 2 are the terminal ones",
        );
    }
    return {
        categoryValue,
        category,
        detailValue,
        detail: detailName(category, detailValue),
        expectedNextOffset,
        durableNextOffset,
        disposition,
    };
}

export function encodeStreamFrame(frame: StreamFrame): Uint8Array {
    const payload = frame.fault !== undefined ? encodeStreamFault(frame.fault) : frame.payload;
    return new Writer(STREAM_HEADER_BYTES + payload.length)
        .u32(frame.sessionId)
        .u64(frame.offset)
        .u16(payload.length)
        .u8(frame.direction)
        .u8(frame.flags)
        .raw(payload)
        .finish();
}

export const encodeStreamFault = (fault: StreamFault): Uint8Array =>
    new Writer(FAULT_BODY_BYTES)
        .u16(fault.categoryValue)
        .u16(fault.detailValue)
        .u64(fault.expectedNextOffset)
        .u64(fault.durableNextOffset)
        .u8(fault.disposition)
        .zeros(3)
        .finish();
