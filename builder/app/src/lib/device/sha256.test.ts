/**
 * The streaming digest, held to WebCrypto's.
 *
 * `Sha256` exists only because `crypto.subtle.digest` cannot be fed incrementally, so the one thing
 * worth proving is that the two agree — for every input length that touches a block boundary, for
 * every slicing of the same message, and for a message long enough that the 64-bit length field is
 * doing real work. A digest that is subtly wrong would not fail loudly; it would reject a perfectly
 * good 200 MB download after four minutes.
 */

import { describe, expect, it } from "vitest";
import { webcrypto } from "node:crypto";

import { Sha256 } from "./sha256";

const subtle = webcrypto.subtle;

async function reference(bytes: Uint8Array): Promise<string> {
    const digest = new Uint8Array(await subtle.digest("SHA-256", bytes));
    return [...digest].map((b) => b.toString(16).padStart(2, "0")).join("");
}

/** Deterministic pseudo-random bytes — a fixed sequence, so a failure is reproducible. */
function noise(len: number, seed = 1): Uint8Array {
    const out = new Uint8Array(len);
    let state = seed >>> 0;
    for (let i = 0; i < len; i++) {
        state = (state * 1664525 + 1013904223) >>> 0;
        out[i] = state >>> 24;
    }
    return out;
}

describe("Sha256", () => {
    it("matches the published test vectors", () => {
        expect(Sha256.hex(new Uint8Array(0))).toBe(
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        );
        expect(Sha256.hex(new TextEncoder().encode("abc"))).toBe(
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        );
    });

    it("matches WebCrypto across every length that touches a block boundary", async () => {
        // 55/56 straddle the point where the padding no longer fits in the final block, 63/64/65
        // the block itself, 119/120 the same pair one block later — the classic off-by-one nest.
        for (const len of [0, 1, 2, 55, 56, 57, 63, 64, 65, 119, 120, 127, 128, 129, 1000, 65_536]) {
            const bytes = noise(len, len + 7);
            expect(Sha256.hex(bytes), `length ${len}`).toBe(await reference(bytes));
        }
    });

    it("is independent of how the message is sliced", async () => {
        const message = noise(100_003, 42);
        const expected = await reference(message);
        for (const slice of [1, 3, 64, 65, 4096, 33_333]) {
            const hash = new Sha256();
            for (let at = 0; at < message.length; at += slice) {
                hash.update(message.subarray(at, Math.min(at + slice, message.length)));
            }
            expect(hash.hex(), `slices of ${slice}`).toBe(expected);
        }
    });

    it("carries the length across more than 2^32 bits", async () => {
        // 600 MB is past the 32-bit bit-length wrap (2^32 bits = 512 MB), which is exactly the
        // range a country-scale map sits in. Fed as one repeated block so the test costs seconds,
        // not gigabytes.
        const block = noise(1 << 20, 5);
        const megabytes = 600;
        const hash = new Sha256();
        for (let i = 0; i < megabytes; i++) hash.update(block);
        // The reference has to see the same message, and Node can hash a 600 MB buffer — but
        // building one would defeat the point of a streaming test, so the comparison is against
        // node:crypto's own streaming hash instead.
        const { createHash } = await import("node:crypto");
        const node = createHash("sha256");
        for (let i = 0; i < megabytes; i++) node.update(block);
        expect(hash.hex()).toBe(node.digest("hex"));
    }, 60_000);
});
