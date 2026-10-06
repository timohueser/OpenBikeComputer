import { PipeError } from "./pipe";
import { RecordError } from "./records";
import { Detail, ErrorCode, ResponseError, opcodeName, refusalName, type Refusal } from "./protocol";

/**
 * Why a device operation failed.
 *
 * The wire's fourteen refusal codes each get their own member, because a client's response to them
 * differs: `no-space` asks the rider to delete something, `busy` retries, `catalog-changed` restarts
 * a listing. The last five are this side of the wire, not the device's.
 */
export type DeviceErrorCode =
    | "link"
    | "timeout"
    | "aborted"
    | "protocol"
    | "unavailable"
    | "unsupported"
    | "invalid-frame"
    | "invalid-request"
    | "not-found"
    | "revision-conflict"
    | "no-space"
    | "checksum"
    | "media-io"
    | "busy"
    | "cancelled"
    | "rejected"
    | "internal"
    | "catalog-changed"
    | "read-only"
    | "device-error";

/** A failure at the protocol layer. `refusal` carries the device's own refusal body. */
export class DeviceError extends Error {
    readonly code: DeviceErrorCode;
    /** The wire refusal, when this error is one. Its `context` is code-scoped. */
    readonly refusal?: Refusal;

    constructor(code: DeviceErrorCode, message: string, options?: { cause?: unknown; refusal?: Refusal }) {
        super(message, options);
        this.name = "DeviceError";
        this.code = code;
        this.refusal = options?.refusal;
    }
}

/** True when LIST cannot name a store but FORMAT is still the intended recovery path. */
export function isFormatRecoveryState(cause: unknown): cause is DeviceError {
    return (
        cause instanceof DeviceError &&
        cause.code === "read-only" &&
        (cause.refusal?.detail === Detail.readOnly.unformatted ||
            cause.refusal?.detail === Detail.readOnly.catalogUnreadable)
    );
}

/** Map a wire refusal onto a caller-facing error, with the sentence that code deserves. */
export function refusalError(refusal: Refusal, opcode: number): DeviceError {
    const what = opcodeName(opcode);
    const named = refusalName(refusal);
    switch (refusal.code) {
        case ErrorCode.Unsupported:
            return new DeviceError(
                "unsupported",
                refusal.detail === Detail.unsupported.wireMajor
                    ? "This device speaks a different protocol version. Update the device firmware, or reload " +
                      "the page for a newer build."
                    : `The device does not support that ${refusal.detail === Detail.unsupported.kind ? "object kind" : "request"} (${named}).`,
                { refusal },
            );
        case ErrorCode.InvalidFrame:
            return new DeviceError("invalid-frame", `The device could not read this ${what} request (${named}).`, {
                refusal,
            });
        case ErrorCode.InvalidRequest:
            return new DeviceError("invalid-request", `The device refused this ${what} request (${named}).`, {
                refusal,
            });
        case ErrorCode.NotFound:
            return new DeviceError("not-found", "The device does not have that object.", { refusal });
        case ErrorCode.RevisionConflict:
            return new DeviceError(
                "revision-conflict",
                refusal.detail === Detail.revisionConflict.headAbsent
                    ? "That object is no longer on the device, so it cannot be replaced."
                    : `That object changed on the device (it is now at revision ${refusal.context}). ` +
                      "Refresh and try again.",
                { refusal },
            );
        case ErrorCode.NoSpace:
            return new DeviceError(
                "no-space",
                refusal.detail === Detail.noSpace.catalogFull
                    ? "The device's catalog is full. Delete something on the device and try again."
                    : `The card needs ${refusal.context} bytes for this and does not have them. ` +
                      "Delete something on the device and try again.",
                { refusal },
            );
        case ErrorCode.ChecksumFailure:
            return new DeviceError(
                "checksum",
                "The device rejected the upload: the payload did not match its checksum. Nothing was " +
                    "stored — try again.",
                { refusal },
            );
        case ErrorCode.MediaIo:
            return new DeviceError("media-io", `The device's card refused a ${named.split("/")[1] ?? "read"}.`, {
                refusal,
            });
        case ErrorCode.Busy:
            return new DeviceError(
                "busy",
                refusal.detail === Detail.busy.holds
                    ? "The device is holding too many objects open. Try again in a moment."
                    : "The device is already busy with another transfer.",
                { refusal },
            );
        case ErrorCode.Cancelled:
            return new DeviceError(
                "cancelled",
                refusal.detail === Detail.cancelled.byDevice
                    ? `The device stopped the ${what}.`
                    : `The ${what} was cancelled. Nothing was stored.`,
                { refusal },
            );
        case ErrorCode.Rejected:
            return new DeviceError(
                "rejected",
                `The device refused that object (${named}, detail ${refusal.detail}).`,
                { refusal },
            );
        case ErrorCode.Internal:
            return new DeviceError("internal", "The device hit a failure it could not classify.", { refusal });
        case ErrorCode.CatalogChanged:
            return new DeviceError(
                "catalog-changed",
                `The device's catalog changed while it was being listed (it is now at commit ${refusal.context}).`,
                { refusal },
            );
        case ErrorCode.ReadOnly:
            return new DeviceError(
                "read-only",
                refusal.detail === Detail.readOnly.unformatted
                    ? "The card in this device is not a flat store. Nothing can be read from it or written to it."
                    : "The device's card is read-only.",
                { refusal },
            );
        default:
            // An unknown code is a failure that cannot be classified, never a success.
            return new DeviceError("device-error", `The device answered the ${what} with ${named}.`, { refusal });
    }
}

/** Normalise a channel-level or unknown failure into a {@link DeviceError}. */
export function asDeviceError(cause: unknown): DeviceError {
    if (cause instanceof DeviceError) return cause;
    if (cause instanceof PipeError) {
        if (cause.code === "aborted") return new DeviceError("aborted", "The transfer was cancelled.", { cause });
        if (cause.code === "closed") return new DeviceError("link", "The device disconnected.", { cause });
        return new DeviceError("device-error", cause.message, { cause });
    }
    if (cause instanceof RecordError || cause instanceof ResponseError) {
        return new DeviceError("protocol", cause.message, { cause });
    }
    return new DeviceError("device-error", cause instanceof Error ? cause.message : String(cause), { cause });
}

