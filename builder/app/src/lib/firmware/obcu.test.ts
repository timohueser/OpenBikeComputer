/**
 * The `UPDATE.BIN` reader, pinned to `specs/vectors/update-container-v2.bin` (the signed container
 * the device actually installs) and to `update-container-v1.bin` for the unsigned-reject case.
 *
 * Those fixtures are the same ones `cargo test -p obc-vectors` and the iOS suite read, so this is the
 * fourth implementation held to one set of bytes rather than a fixture captured from this code.
 * The rejection cases matter as much as the happy one: everything this reader refuses is a file
 * that would otherwise be pushed down a cable for a minute and then refused by the device.
 */

import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

import { Crc32 } from "../usb/crc32";
import { FirmwareFileError, OBCU_HEADER_LEN, OBCU_SIG_LEN, readUpdateImage } from "./obcu";

function repoRoot(): string {
    let dir = dirname(fileURLToPath(import.meta.url));
    for (let up = 0; up < 12; up++) {
        if (existsSync(join(dir, "specs", "vectors", "manifest.json"))) return dir;
        dir = dirname(dir);
    }
    throw new Error("could not locate the repo root from " + import.meta.url);
}

/** The signed (v2) container — what a release publishes and what this reader must accept. */
const CONTAINER = new Uint8Array(
    readFileSync(join(repoRoot(), "specs/vectors", "update-container-v2.bin")),
);

/** The unsigned (v1) container — a shape the device refuses, so this reader must too (§1.4). */
const CONTAINER_V1 = new Uint8Array(
    readFileSync(join(repoRoot(), "specs/vectors", "update-container-v1.bin")),
);

/** A copy with one byte changed — every rejection case is a one-byte edit of a valid file. */
function edited(at: number, value: number): Uint8Array {
    const copy = Uint8Array.from(CONTAINER);
    copy[at] = value;
    return copy;
}

/**
 * A copy whose header was mutated **and re-sealed** — the header CRC recomputed over `0..60`, so the
 * mutation is tested by the check it targets rather than dying at the CRC. This is also the realistic
 * attacker: nothing stops them fixing a CRC.
 */
function resealed(mutate: (header: DataView) => void): Uint8Array {
    const copy = Uint8Array.from(CONTAINER);
    mutate(new DataView(copy.buffer, copy.byteOffset, OBCU_HEADER_LEN));
    new DataView(copy.buffer, copy.byteOffset, OBCU_HEADER_LEN).setUint32(60, Crc32.of(copy.subarray(0, 60)), true);
    return copy;
}

function rejection(bytes: Uint8Array): FirmwareFileError {
    try {
        readUpdateImage(bytes);
    } catch (e) {
        expect(e, "every rejection is a FirmwareFileError").toBeInstanceOf(FirmwareFileError);
        return e as FirmwareFileError;
    }
    throw new Error("expected a rejection");
}

describe("readUpdateImage", () => {
    it("reads the checked-in signed container's header", () => {
        const { image, container } = readUpdateImage(CONTAINER);
        // The numbers the vector manifest records for this fixture.
        expect(image.version).toBe("1.2.0+abc1234");
        expect(image.imageLen).toBe(128);
        expect(image.imageCrc32).toBe(0x5b990292);
        expect(image.sigScheme).toBe(1);
        expect(image.sigLen).toBe(OBCU_SIG_LEN);
        expect(image.containerLen).toBe(OBCU_HEADER_LEN + 128 + OBCU_SIG_LEN);
        expect(container.length).toBe(256);
    });

    it("carries the signature trailer into the container it sends", () => {
        // The regression that would break every USB update: truncating at `64 + imageLen` drops the
        // trailer, and the device then refuses the file it received as truncated (§1.4 step 4).
        const { container } = readUpdateImage(CONTAINER);
        expect(container).toEqual(CONTAINER);
        expect(container.subarray(OBCU_HEADER_LEN + 128)).toEqual(CONTAINER.subarray(OBCU_HEADER_LEN + 128));
    });

    it("drops trailing slack rather than sending it", () => {
        // `OBCU_Spec.md` §1.1/§2.3: bytes past the container are ignored. Announcing them would make
        // the device store a length that disagrees with the header it just verified.
        const padded = new Uint8Array(CONTAINER.length + 512);
        padded.set(CONTAINER, 0);
        const { container } = readUpdateImage(padded);
        expect(container.length).toBe(256);
        expect(container).toEqual(CONTAINER);
    });

    it("refuses an unsigned container before spending the transfer on it", () => {
        // §1.4: the device installs signed containers only. Learning that here costs a second;
        // learning it after the upload costs a minute of cable time.
        const e = rejection(CONTAINER_V1);
        expect(e.code).toBe("unsigned");
        expect(e.message).toContain("not signed");
    });

    it("refuses a signature scheme it does not know", () => {
        expect(rejection(resealed((h) => h.setUint16(48, 0x99, true))).code).toBe("unsigned");
        expect(rejection(resealed((h) => h.setUint16(50, 32, true))).code).toBe("unsigned");
    });

    it("refuses a container whose trailer is missing or short", () => {
        expect(rejection(CONTAINER.subarray(0, OBCU_HEADER_LEN + 128)).code).toBe("truncated");
        expect(rejection(CONTAINER.subarray(0, CONTAINER.length - 1)).code).toBe("truncated");
    });

    it("refuses a file that is not a container at all", () => {
        expect(rejection(new Uint8Array(10)).code).toBe("not-obcu");
        expect(rejection(edited(0, 0x41)).code).toBe("not-obcu");
    });

    it("refuses a header version it does not implement", () => {
        // §1.1: a version change is a hard reject, never a silent migration. The header CRC has to
        // stay valid for this to test what it says it does, so the check order is what is asserted.
        expect(rejection(edited(4, 2)).code).toBe("not-obcu");
    });

    it("refuses a damaged header", () => {
        expect(rejection(edited(60, CONTAINER[60] ^ 0xff)).code).toBe("header-crc");
    });

    it("refuses a damaged image", () => {
        expect(rejection(edited(OBCU_HEADER_LEN + 3, CONTAINER[OBCU_HEADER_LEN + 3] ^ 0xff)).code).toBe(
            "image-crc",
        );
    });

    it("refuses a truncated container", () => {
        expect(rejection(CONTAINER.subarray(0, 150)).code).toBe("truncated");
    });
});
