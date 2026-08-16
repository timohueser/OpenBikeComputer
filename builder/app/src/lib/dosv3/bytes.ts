/**
 * Bounds-checked little-endian readers and writers.
 *
 * Device_Object_Protocol_v3.md §1: integers are unsigned little-endian, every multi-byte field is
 * byte-packed at exactly its stated offset, and no wire structure contains alignment padding. So a
 * `Cursor` walks a buffer field by field, and every read past the end is a typed rejection rather
 * than the `undefined` a raw index would hand back.
 */

import { reject, type CategoryName } from "./result";

/** How a reader reports running off the end of its buffer. Framing faults and body faults differ. */
export interface OverrunReason {
    readonly category: CategoryName;
    readonly detail: string;
}

export const TRUNCATED: OverrunReason = { category: "invalidFrame", detail: "truncated" };
export const NESTED_LENGTH: OverrunReason = { category: "invalidDescriptor", detail: "nestedLength" };

export class Cursor {
    private readonly view: DataView;
    private offset = 0;

    constructor(
        private readonly bytes: Uint8Array,
        private readonly overrun: OverrunReason = TRUNCATED,
    ) {
        this.view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    }

    get position(): number {
        return this.offset;
    }

    get remaining(): number {
        return this.bytes.length - this.offset;
    }

    private need(count: number): number {
        if (this.remaining < count) {
            reject(
                this.overrun.category,
                this.overrun.detail,
                `wanted ${count} more bytes at offset ${this.offset}, ${this.remaining} remain`,
            );
        }
        const at = this.offset;
        this.offset += count;
        return at;
    }

    u8(): number {
        return this.view.getUint8(this.need(1));
    }

    u16(): number {
        return this.view.getUint16(this.need(2), true);
    }

    u32(): number {
        return this.view.getUint32(this.need(4), true);
    }

    u64(): bigint {
        return this.view.getBigUint64(this.need(8), true);
    }

    i32(): number {
        return this.view.getInt32(this.need(4), true);
    }

    i64(): bigint {
        return this.view.getBigInt64(this.need(8), true);
    }

    take(count: number): Uint8Array {
        const at = this.need(count);
        return this.bytes.slice(at, at + count);
    }

    /** Reads `count` bytes and rejects unless every one of them is zero (§1 reserved-field rule). */
    zeros(count: number, what: string): void {
        const at = this.need(count);
        for (let i = 0; i < count; i++) {
            if (this.bytes[at + i] !== 0) {
                reject("invalidDescriptor", "reservedBits", `${what} is reserved and encoded zero`);
            }
        }
    }

    /** Rejects unless the cursor sits exactly at the end of its buffer. */
    end(what: string, reason: OverrunReason = { category: "invalidFrame", detail: "trailingBytes" }): void {
        if (this.remaining !== 0) {
            reject(reason.category, reason.detail, `${what} has ${this.remaining} trailing bytes`);
        }
    }
}

export class Writer {
    private bytes: Uint8Array;
    private view: DataView;
    private offset = 0;

    constructor(capacity = 64) {
        this.bytes = new Uint8Array(Math.max(capacity, 16));
        this.view = new DataView(this.bytes.buffer);
    }

    private room(count: number): number {
        if (this.offset + count > this.bytes.length) {
            const grown = new Uint8Array(Math.max(this.bytes.length * 2, this.offset + count));
            grown.set(this.bytes.subarray(0, this.offset));
            this.bytes = grown;
            this.view = new DataView(grown.buffer);
        }
        const at = this.offset;
        this.offset += count;
        return at;
    }

    u8(value: number): this {
        this.view.setUint8(this.room(1), value);
        return this;
    }

    u16(value: number): this {
        this.view.setUint16(this.room(2), value, true);
        return this;
    }

    u32(value: number): this {
        this.view.setUint32(this.room(4), value, true);
        return this;
    }

    u64(value: bigint): this {
        this.view.setBigUint64(this.room(8), value, true);
        return this;
    }

    i32(value: number): this {
        this.view.setInt32(this.room(4), value, true);
        return this;
    }

    i64(value: bigint): this {
        this.view.setBigInt64(this.room(8), value, true);
        return this;
    }

    raw(value: Uint8Array): this {
        // `room` may replace `this.bytes` with a grown buffer, so claim the space first and only
        // then read the field back — evaluating `this.bytes` before the call writes into the old one.
        const at = this.room(value.length);
        this.bytes.set(value, at);
        return this;
    }

    zeros(count: number): this {
        this.room(count);
        return this;
    }

    get length(): number {
        return this.offset;
    }

    finish(): Uint8Array {
        return this.bytes.slice(0, this.offset);
    }
}

const HEX = "0123456789abcdef";

export function bytesToHex(bytes: Uint8Array): string {
    let out = "";
    for (const byte of bytes) out += HEX[byte >> 4] + HEX[byte & 0x0f];
    return out;
}

export function hexToBytes(hex: string): Uint8Array {
    if (hex.length % 2 !== 0) throw new Error("hex string has an odd length");
    const out = new Uint8Array(hex.length / 2);
    for (let i = 0; i < out.length; i++) {
        const byte = Number.parseInt(hex.slice(i * 2, i * 2 + 2), 16);
        if (Number.isNaN(byte)) throw new Error(`"${hex}" is not hexadecimal`);
        out[i] = byte;
    }
    return out;
}

export function bytesEqual(a: Uint8Array, b: Uint8Array): boolean {
    if (a.length !== b.length) return false;
    for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
    return true;
}
