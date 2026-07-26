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
export const OBCU_MAX_IMAGE_LEN = 1_480_000;
export const OBCU_MAX_CONTAINER_LEN = OBCU_MAX_IMAGE_LEN + OBCU_HEADER_LEN;

const MAGIC = 0x4f424355; // "OBCU"

/** What the header says about the image behind it. */
export interface UpdateImage {
    /** The `git describe` version string the image was wrapped with, trailing NULs trimmed. */
    readonly version: string;
    /** Bytes of raw image following the header. */
    readonly imageLen: number;
    readonly imageCrc32: number;
    /** `64 + imageLen` — what the `fwImage` transfer announces (§7.6). */
    readonly containerLen: number;
}

export type FirmwareFileErrorCode = "not-obcu" | "header-crc" | "truncated" | "image-crc" | "too-large";

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
 * Decode and fully verify an update container.
 *
 * Returns the container **exactly as it will be sent**: `64 + imageLen` bytes, with any trailing
 * slack dropped. Bytes past the image are ignored by the spec (§2.3), and sending them would make
 * the announced length disagree with what the device stores for no benefit.
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
    const containerLen = OBCU_HEADER_LEN + imageLen;
    if (bytes.length < containerLen) {
        throw new FirmwareFileError(
            "truncated",
            `That update is cut short: the header announces ${imageLen} bytes of image, the file holds ` +
                `${bytes.length - OBCU_HEADER_LEN}. Download it again.`,
        );
    }
    const imageCrc32 = view.getUint32(12, true);
    const container = bytes.subarray(0, containerLen);
    if (Crc32.of(container.subarray(OBCU_HEADER_LEN)) !== imageCrc32) {
        throw new FirmwareFileError("image-crc", "That update failed its checksum. Download it again.");
    }
    return {
        image: {
            version: trimNul(bytes.subarray(16, 48)),
            imageLen,
            imageCrc32,
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
