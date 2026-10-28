/**
 * The streaming digest, held to WebCrypto's.
 *
 * `Sha256` exists only because `crypto.subtle.digest` cannot be fed incrementally, so the one thing
 * worth proving is that the two agree — for every input length that touches a block boundary and for
 * every slicing of the same message. A digest that is subtly wrong would not fail loudly; it would
 * reject a perfectly good 200 MB download after four minutes. The 64-bit length counter is the `sha2`
 * crate's, behind a wrapper that keeps no length of its own.
 */

import { describe, expect, it } from "vitest";
import { webcrypto } from "node:crypto";

import { Sha256 } from "../core/hash";

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
    it("rejects use after finalizing the Rust state", () => {
        const hash = new Sha256();
        hash.digest();
        expect(() => hash.digest()).toThrow("Sha256.digest called twice");
        expect(() => hash.update(new Uint8Array())).toThrow("Sha256.update after digest()");
    });
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
});
