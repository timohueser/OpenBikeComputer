/**
 * The CRC-32 constants of interface spec §6, pinned as literals.
 *
 * `vectors.test.ts` asserts the same things against `specs/vectors/`; this file states them
 * without any file to read, so a hasher that quietly became CRC-32C — same shape, same API, wrong
 * answers — fails here with the spec paragraph next to it rather than as a byte mismatch four
 * layers away.
 */

import { describe, expect, it } from "vitest";

import { Crc32 } from "./crc32";

const bytes = (s: string): Uint8Array => new TextEncoder().encode(s);

describe("CRC-32/IEEE", () => {
    it("matches the spec's check value", () => {
        // §6: reflected, polynomial 0xEDB88320, init and xorout 0xFFFFFFFF.
        expect(Crc32.of(bytes("123456789"))).toBe(0xcbf43926);
    });

    it("is zero over no bytes", () => {
        expect(Crc32.of(new Uint8Array())).toBe(0);
    });

    it("returns an unsigned value, never a negative int32", () => {
        // The descriptor carries the CRC as a `u32`, and a sign-extended one would encode as a
        // different four bytes. `0xEDB88320`-family CRCs run past 2^31 routinely.
        const value = Crc32.of(bytes("The quick brown fox jumps over the lazy dog"));
        expect(value).toBe(0x414fa339);
        expect(Crc32.of(new Uint8Array([0xff, 0xff, 0xff, 0xff]))).toBeGreaterThan(0x7fffffff);
    });

    it("gives the same answer however the bytes are split", () => {
        const data = Uint8Array.from({ length: 259 }, (_, i) => (i * 7 + 3) & 0xff);
        const whole = Crc32.of(data);
        for (let split = 0; split <= data.length; split++) {
            const h = new Crc32();
            h.update(data.subarray(0, split));
            h.update(data.subarray(split));
            expect(h.value(), `split at ${split}`).toBe(whole);
        }
    });

    it("can be read mid-stream and reset for a retry", () => {
        const h = new Crc32();
        h.update(bytes("1234"));
        const partial = h.value();
        h.update(bytes("56789"));
        expect(h.value()).toBe(0xcbf43926);
        expect(partial).not.toBe(h.value());
        h.reset();
        h.update(bytes("123456789"));
        expect(h.value()).toBe(0xcbf43926);
    });
});
