/**
 * The `UPDATE.BIN` reader, pinned to `protocol-vectors/update-container-v1.bin`.
 *
 * That fixture is the same one `cargo test -p obc-vectors` and the iOS suite read, so this is the
 * fourth implementation held to one set of bytes rather than a fixture captured from this code.
 * The rejection cases matter as much as the happy one: everything this reader refuses is a file
 * that would otherwise be pushed down a cable for a minute and then refused by the device.
 */

import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

import { FirmwareFileError, OBCU_HEADER_LEN, readUpdateImage } from "./obcu";

function repoRoot(): string {
    let dir = dirname(fileURLToPath(import.meta.url));
    for (let up = 0; up < 12; up++) {
        if (existsSync(join(dir, "protocol-vectors", "manifest.json"))) return dir;
        dir = dirname(dir);
    }
    throw new Error("could not locate the repo root from " + import.meta.url);
}

const CONTAINER = new Uint8Array(
    readFileSync(join(repoRoot(), "protocol-vectors", "update-container-v1.bin")),
);

/** A copy with one byte changed — every rejection case is a one-byte edit of a valid file. */
function edited(at: number, value: number): Uint8Array {
    const copy = Uint8Array.from(CONTAINER);
    copy[at] = value;
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
    it("reads the checked-in container's header", () => {
        const { image, container } = readUpdateImage(CONTAINER);
        // The numbers the vector manifest records for this fixture.
        expect(image.version).toBe("1.2.0+abc1234");
        expect(image.imageLen).toBe(128);
        expect(image.imageCrc32).toBe(0x5b990292);
        expect(image.containerLen).toBe(OBCU_HEADER_LEN + 128);
        expect(container.length).toBe(192);
    });

    it("drops trailing slack rather than sending it", () => {
        // `OBCU_Spec.md` §2.3: bytes past `64 + image_len` are ignored. Announcing them would make
        // the device store a length that disagrees with the header it just verified.
        const padded = new Uint8Array(CONTAINER.length + 512);
        padded.set(CONTAINER, 0);
        const { container } = readUpdateImage(padded);
        expect(container.length).toBe(192);
        expect(container).toEqual(CONTAINER);
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
