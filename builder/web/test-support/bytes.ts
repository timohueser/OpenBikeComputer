import { expect } from "vitest";

/** `toEqual` walks a typed array one element at a time, which takes seconds for a megabyte. */
export function expectBytes(actual: Uint8Array | null, expected: Uint8Array): void {
    expect(actual?.length).toBe(expected.length);
    expect(actual!.findIndex((byte, i) => byte !== expected[i]), "the first byte that differs").toBe(-1);
}
