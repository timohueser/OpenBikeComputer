/**
 * The `UPDATE.BIN` container header, read in the browser (`OBCU_Spec.md` §1).
 *
 * The page checks an update image before it spends minutes pushing it down a cable, and before the
 * device spends its own time scanning a file it will reject. Everything here is the spec's own
 * decode rule, not a second opinion about it: bad magic, a header version other than `1`, or a
 * header CRC that does not match bytes `0..60` means "not a valid container", and the raw image's
 * CRC-32 is checked **separately** against the bytes that follow — which is exactly what the
 * device-side armer does before it arms anything.
 *
 * This is a *pre-flight*, and deliberately not a substitute for any device-side check. #728's DFU
 * work turns on verify-before-erase happening on the device, over the bytes actually on the card;
 * a browser that validated a file and then uploaded it corrupted would still be caught there. What
 * the pre-flight buys is the difference between "that file isn't a firmware update" in a second and
 * the same answer after a transfer.
 *
 * **OBCU v2 (#997) — the signature is the device's business, the trailer is ours.** A signed
 * container is the header, the image, and a 64-byte Ed25519 trailer past `64 + imageLen`
 * (`OBCU_Spec.md` §1.3). This page does **not** verify that signature: the trusted key lives in the
 * firmware, not in a web page, and a browser-side "valid" would mean nothing the device didn't
 * re-establish anyway. What it must do is *carry the trailer intact* — the container it announces and
 * uploads has to be the whole file, or the device receives a file whose signature it cannot find and
 * refuses it as truncated. It also refuses an **unsigned** container up front, because the device
 * will (§1.4) and finding that out before the transfer is the entire point of a pre-flight.
 */

import { Crc32 } from "../usb/crc32";
import { viewOf } from "../usb/protocol";

/** The fixed container header (§1.1). */
export const OBCU_HEADER_LEN = 64;

/** The only header version readers accept (§1.1 — a version change is a hard reject). */
export const OBCU_HEADER_VERSION = 1;

/**
 * The app-slot ceiling on the raw image (§1.1), and the container ceiling that follows from it.
 *
 * The L15 DK's number. The LM20's larger slot is a "future mechanical bump" per the spec, and this
 * page is not where that decision gets made — a device announces its own reject if the two ever
 * disagree, which is why the check here is a courtesy rather than the gate.
 */
/** `sig_scheme` (§1.1, header bytes 48..50): 0 = unsigned/v1, 1 = Ed25519/v2. */
export const OBCU_SIG_SCHEME_NONE = 0;
export const OBCU_SIG_SCHEME_ED25519 = 1;
/** Bytes of the Ed25519 signature trailer (§1.3). */
export const OBCU_SIG_LEN = 64;

export const OBCU_MAX_IMAGE_LEN = 1_480_000;

const MAGIC = 0x4f424355; // "OBCU"

/** What the header says about the image behind it. */
export interface UpdateImage {
    /** The `git describe` version string the image was wrapped with, trailing NULs trimmed. */
    readonly version: string;
    /** Bytes of raw image following the header. */
    readonly imageLen: number;
    readonly imageCrc32: number;
    /** `1` for an Ed25519-signed v2 container (§1.1). */
    readonly sigScheme: number;
    /** Bytes of signature trailer after the image — `64` for Ed25519, `0` unsigned. */
    readonly sigLen: number;
    /** `64 + imageLen + sigLen` — what the `fwImage` transfer announces (§7.6). */
    readonly containerLen: number;
}

export type FirmwareFileErrorCode =
    | "not-obcu"
    | "header-crc"
    | "truncated"
    | "image-crc"
    | "too-large"
    | "unsigned";

/** A rejected update file, with a sentence for the rider and a code for the caller. */
export class FirmwareFileError extends Error {
    readonly code: FirmwareFileErrorCode;

    constructor(code: FirmwareFileErrorCode, message: string) {
        super(message);
        this.name = "FirmwareFileError";
        this.code = code;
    }
}

/**
 * Decode and fully verify an update container (everything but the signature itself — see the module
 * note).
 *
 * Returns the container **exactly as it will be sent**: `64 + imageLen + sigLen` bytes, with any
 * trailing slack past that dropped. The signature trailer is part of the container and must survive
 * the trip; only FAT/download slack beyond it is dropped, because announcing bytes the device ignores
 * would make the transfer length disagree with the file for no benefit.
 */
export function readUpdateImage(bytes: Uint8Array): { image: UpdateImage; container: Uint8Array } {
    if (bytes.length < OBCU_HEADER_LEN) {
        throw new FirmwareFileError(
            "not-obcu",
            `That file is ${bytes.length} bytes — an update starts with a ${OBCU_HEADER_LEN}-byte header.`,
        );
    }
    const view = viewOf(bytes);
    if (view.getUint32(0, false) !== MAGIC) {
        throw new FirmwareFileError("not-obcu", "That file is not a firmware update — it should be an UPDATE.BIN.");
    }
    const headerVersion = view.getUint16(4, true);
    if (headerVersion !== OBCU_HEADER_VERSION) {
        throw new FirmwareFileError(
            "not-obcu",
            `That update uses container version ${headerVersion}; this page writes version ` +
                `${OBCU_HEADER_VERSION}. Use a build that matches your firmware.`,
        );
    }
    const headerCrc = Crc32.of(bytes.subarray(0, 60));
    if (headerCrc !== view.getUint32(60, true)) {
        throw new FirmwareFileError("header-crc", "That update's header is damaged. Download it again.");
    }

    const imageLen = view.getUint32(8, true);
    if (imageLen > OBCU_MAX_IMAGE_LEN) {
        throw new FirmwareFileError(
            "too-large",
            `That image is ${imageLen} bytes; the device's update slot holds ${OBCU_MAX_IMAGE_LEN}.`,
        );
    }
    // §1.1: the scheme marker in v1's reserved bytes. §1.4: the device installs signed containers
    // only, so an unsigned one is refused here rather than after the upload. This page cannot check
    // the signature — the key is in the firmware — but it must know the trailer is there and send it.
    const sigScheme = view.getUint16(48, true);
    const sigLen = view.getUint16(50, true);
    if (sigScheme !== OBCU_SIG_SCHEME_ED25519 || sigLen !== OBCU_SIG_LEN) {
        throw new FirmwareFileError(
            "unsigned",
            sigScheme === OBCU_SIG_SCHEME_NONE
                ? "That update file is not signed, and the device only installs signed updates. " +
                  "Download the release build rather than a local one."
                : `That update uses signature scheme ${sigScheme}, which this device's firmware ` +
                  "does not verify. Use a build that matches your firmware.",
        );
    }

    const containerLen = OBCU_HEADER_LEN + imageLen + sigLen;
    if (bytes.length < containerLen) {
        throw new FirmwareFileError(
            "truncated",
            `That update is cut short: the header announces ${imageLen} bytes of image plus a ` +
                `${sigLen}-byte signature, the file holds ${bytes.length - OBCU_HEADER_LEN} after its ` +
                "header. Download it again.",
        );
    }
    const imageCrc32 = view.getUint32(12, true);
    const container = bytes.subarray(0, containerLen);
    if (Crc32.of(container.subarray(OBCU_HEADER_LEN, OBCU_HEADER_LEN + imageLen)) !== imageCrc32) {
        throw new FirmwareFileError("image-crc", "That update failed its checksum. Download it again.");
    }
    return {
        image: {
            version: trimNul(bytes.subarray(16, 48)),
            imageLen,
            imageCrc32,
            sigScheme,
            sigLen,
            containerLen,
        },
        container,
    };
}

function trimNul(bytes: Uint8Array): string {
    let end = bytes.length;
    while (end > 0 && bytes[end - 1] === 0) end--;
    return new TextDecoder().decode(bytes.subarray(0, end));
}
