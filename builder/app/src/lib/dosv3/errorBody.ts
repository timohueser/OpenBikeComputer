/**
 * ErrorBody, Device_Object_Protocol_v3.md §12.
 *
 * Two rules shape this decoder more than the byte table does.
 *
 * The presence matrix **binds senders only**: "A decoder MUST NOT reject an ErrorBody because an
 * optional field is present where it expected none, or absent where the category would normally
 * require one." That is what makes the retained-Aborted replays of §11 — durable records whose
 * presence bits are forced clear and whose guidance is forced to reject-permanently — decodable
 * without a special case, so nothing here checks a category against its required presence.
 *
 * Diagnostic text is **never** a reason to reject: "refusing an error body would destroy the only
 * report of a real failure to protect a field that drives nothing." Only its length field is
 * structural. So the text is carried as bytes, rendered lossily on demand, and an invalid-UTF-8
 * body decodes.
 */

import { Cursor, Writer } from "./bytes";
import { renderDiagnosticText } from "./metadata";
import { OBJECT_KIND_NAME, semanticDetailName, type ObjectKindName } from "./registry";
import {
    CATEGORY,
    CATEGORY_NAME,
    MAX_GUIDANCE,
    MAX_OWNER,
    detailName,
    reject,
    type CategoryName,
    type DosError,
} from "./result";

export const ERROR_BODY_PREFIX_BYTES = 48;
export const MAX_ERROR_TEXT_BYTES = 64;

/** §12 presence bits. Bits 7..15 are zero. */
export const PRESENCE = {
    retryDelay: 1 << 0,
    expectedOffset: 1 << 1,
    currentRevision: 1 << 2,
    requiredBytes: 1 << 3,
    availableBytes: 1 << 4,
    durableClaimExists: 1 << 5,
    claimIsTerminal: 1 << 6,
} as const;
export const PRESENCE_MASK = 0x7f;

export interface ErrorBody {
    readonly categoryValue: number;
    /** The registered name, or "unknown" for a category above the v3.0 table. */
    readonly category: CategoryName | "unknown";
    /** Detail namespace: common `0`, or the ObjectKind that owns a semanticValidation rule. */
    readonly namespace: number;
    readonly namespaceKind?: ObjectKindName;
    readonly detailValue: number;
    readonly detail: string;
    readonly guidance: number;
    readonly owner: number;
    readonly presence: number;
    readonly retryAfterMs: number;
    readonly expectedOffset: bigint;
    readonly currentRevision: bigint;
    readonly requiredBytes: bigint;
    readonly availableBytes: bigint;
    readonly text: Uint8Array;
}

export function decodeErrorBody(bytes: Uint8Array): ErrorBody {
    const cursor = new Cursor(bytes);
    const categoryValue = cursor.u16();
    const namespace = cursor.u16();
    const detailValue = cursor.u16();
    const guidance = cursor.u8();
    const owner = cursor.u8();
    const presence = cursor.u16();
    const retryAfterMs = cursor.u32();
    const expectedOffset = cursor.u64();
    const currentRevision = cursor.u64();
    const requiredBytes = cursor.u64();
    const availableBytes = cursor.u64();
    const textLength = cursor.u8();
    cursor.zeros(1, "ErrorBody byte 47");

    if (categoryValue === 0) {
        reject("invalidDescriptor", "unknownEnum", "category 0 is reserved and invalid, not an unknown future one");
    }
    if (guidance > MAX_GUIDANCE) reject("invalidDescriptor", "unknownEnum", `retry guidance ${guidance} is not registered`);
    if (owner > MAX_OWNER) reject("invalidDescriptor", "unknownEnum", `owner ${owner} is not registered`);
    if ((presence & ~PRESENCE_MASK) !== 0) reject("invalidDescriptor", "reservedBits", "presence bits 7..15 are zero");
    if ((presence & PRESENCE.claimIsTerminal) !== 0 && (presence & PRESENCE.durableClaimExists) === 0) {
        reject("invalidDescriptor", "invalidCombination", "claim-is-terminal is meaningful only with durable-claim");
    }

    const category = CATEGORY_NAME.get(categoryValue) ?? "unknown";
    let namespaceKind: ObjectKindName | undefined;
    if (namespace !== 0) {
        if (categoryValue !== CATEGORY.semanticValidation) {
            reject("invalidDescriptor", "invalidCombination", "categories other than semanticValidation use namespace 0");
        }
        namespaceKind = OBJECT_KIND_NAME.get(namespace);
        if (namespaceKind === undefined) {
            reject("invalidDescriptor", "unknownEnum", `detail namespace ${namespace} is not a registered ObjectKind`);
        }
    }

    if (textLength > MAX_ERROR_TEXT_BYTES) {
        reject("invalidFrame", "payloadLength", `error text is at most ${MAX_ERROR_TEXT_BYTES} bytes`);
    }
    if (cursor.remaining !== textLength) {
        reject("invalidFrame", "payloadLength", "the text length disagrees with the body length");
    }
    const text = cursor.take(textLength);

    const detail =
        namespaceKind !== undefined
            ? semanticDetailName(namespaceKind, detailValue)
            : category === "unknown"
              ? "unknown"
              : detailName(category, detailValue);

    return {
        categoryValue,
        category,
        namespace,
        namespaceKind,
        detailValue,
        detail,
        guidance,
        owner,
        presence,
        retryAfterMs,
        expectedOffset,
        currentRevision,
        requiredBytes,
        availableBytes,
        text,
    };
}

export function encodeErrorBody(body: ErrorBody): Uint8Array {
    return new Writer(ERROR_BODY_PREFIX_BYTES + body.text.length)
        .u16(body.categoryValue)
        .u16(body.namespace)
        .u16(body.detailValue)
        .u8(body.guidance)
        .u8(body.owner)
        .u16(body.presence)
        .u32(body.retryAfterMs)
        .u64(body.expectedOffset)
        .u64(body.currentRevision)
        .u64(body.requiredBytes)
        .u64(body.availableBytes)
        .u8(body.text.length)
        .zeros(1)
        .raw(body.text)
        .finish();
}

/** True when the body carries the §11 signature of a replayed terminal result: bits 5 and 6 set. */
export const isRetainedTerminalReplay = (body: ErrorBody): boolean =>
    (body.presence & (PRESENCE.durableClaimExists | PRESENCE.claimIsTerminal)) ===
    (PRESENCE.durableClaimExists | PRESENCE.claimIsTerminal);

/** The lossy rendering §12 mandates. Never parsed, never matched on, never behaviour-bearing. */
export const errorText = (body: ErrorBody): string => renderDiagnosticText(body.text);

/** Projects a decoded ErrorBody onto this module's own failure vocabulary. */
export function asDosError(body: ErrorBody): DosError {
    return {
        category: body.category === "unknown" ? "internal" : body.category,
        categoryValue: body.categoryValue,
        detail: body.detail,
        detailValue: body.detailValue,
        message: errorText(body),
        namespace: body.namespace,
    };
}
