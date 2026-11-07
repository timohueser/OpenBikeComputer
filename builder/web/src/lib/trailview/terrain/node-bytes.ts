// The bytes of what the stamp worker posts for a node: a baked node holds them, and the viewer reads them back.

/** The levels whose whole node the stamp layer holds, stamped with the ground plan. It holds the owner level's structure meshes too. */
export const BAKED = { from: 9, to: 12 };

/** The typed arrays that the bytes hold, by their tags. */
const ARRAYS = { Uint8Array, Uint16Array, Uint32Array, Int32Array, Float32Array, Float64Array };
type Tag = keyof typeof ARRAYS | 'ArrayBuffer';
/** A binary value in the JSON: its tag, its offset in the binary part and its bytes. */
interface Slot { $: Tag; at: number; n: number }

const align = (n: number) => (n + 7) & ~7;

/**
 * The bytes of `value`, plain objects, arrays, booleans, null, finite numbers, strings, ArrayBuffers and
 * typed arrays: the length of the JSON as a little-endian u32, the JSON, then each binary value at an
 * offset of 8 bytes. A value that JSON changes (NaN, ±Infinity, -0, a class instance) throws: decodeNode gives back `value` exactly.
 */
export function encodeNode(value: unknown): Uint8Array<ArrayBuffer> {
    const parts: Uint8Array[] = [];
    let end = 0;
    const json = new TextEncoder().encode(JSON.stringify(value, (_, item: unknown) => {
        if (typeof item === 'number' && (!Number.isFinite(item) || Object.is(item, -0))) throw new Error(`encodeNode: ${Object.is(item, -0) ? '-0' : item}`);
        const tag = item instanceof ArrayBuffer ? 'ArrayBuffer' : ArrayBuffer.isView(item) ? item.constructor.name : undefined;
        if (!tag && item !== null && typeof item === 'object' && !Array.isArray(item) && Object.getPrototypeOf(item) !== Object.prototype) throw new Error(`encodeNode: ${item.constructor.name}`);
        if (!tag) return item;
        if (tag !== 'ArrayBuffer' && !Object.hasOwn(ARRAYS, tag)) throw new Error(`encodeNode: ${tag}`);
        const bytes = item instanceof ArrayBuffer ? new Uint8Array(item) : new Uint8Array((item as ArrayBufferView).buffer, (item as ArrayBufferView).byteOffset, (item as ArrayBufferView).byteLength);
        const slot: Slot = { $: tag as Tag, at: end, n: bytes.byteLength };
        parts.push(bytes);
        end = align(end + bytes.byteLength);
        return slot;
    }));
    const base = align(4 + json.byteLength), out = new Uint8Array(base + end);
    new DataView(out.buffer).setUint32(0, json.byteLength, true);
    out.set(json, 4);
    let at = base;
    for (const bytes of parts) {
        out.set(bytes, at);
        at += align(bytes.byteLength);
    }
    return out;
}

/** The value of bytes from encodeNode. Each binary value has an ArrayBuffer of its own. */
export function decodeNode(bytes: Uint8Array): unknown {
    const length = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getUint32(0, true), base = bytes.byteOffset + align(4 + length);
    const json = new TextDecoder().decode(bytes.subarray(4, 4 + length));
    return JSON.parse(json, (_, item) => {
        if (item === null || typeof item !== 'object' || typeof item.$ !== 'string') return item;
        const { $, at, n } = item as Slot, buffer = bytes.buffer.slice(base + at, base + at + n) as ArrayBuffer;
        return $ === 'ArrayBuffer' ? buffer : new ARRAYS[$](buffer);
    });
}
