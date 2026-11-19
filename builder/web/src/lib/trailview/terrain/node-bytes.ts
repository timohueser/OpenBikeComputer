// The bytes of what the stamp worker posts for a node: a baked node holds them, and the viewer reads them back.
import { MeshoptDecoder } from 'meshoptimizer/decoder';

/**
 * The levels whose whole node the stamp layer holds, stamped with the ground plan. It holds the structure
 * meshes of each owner node that has any, and at zoom 13 the u16 mask of the owner nodes that it holds.
 */
export const BAKED = { from: 9, to: 12 };

/** The structure meshes of an owner node without a structure, as the worker posts them. */
export const EMPTY_MESHES = { far: { structures: null, anchored: null }, near: { structures: null, anchored: null }, anchors: null, points: null };

/**
 * A mesh `{ vertices, indices, ...rest }` in the layer: `$mesh` and the rest. The distinct vertices `v` in the order
 * of their first use, `w` of `s` bytes each, meshopt-coded; with `xy` their first two int32 words are int16 from
 * that point. `r` holds, meshopt-coded, the distinct vertex of each of the `n` vertices (null when all differ),
 * `i` the `ni` indices.
 */
export interface PackedMesh { n: number; w: number; s: number; xy: [number, number] | null; v: Uint8Array; r: Uint8Array | null; ni: number; i: Uint8Array }

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

/** A sequence of u32 from meshopt's index sequence code. */
function sequence(count: number, code: Uint8Array): Uint32Array<ArrayBuffer> {
    const out = new Uint32Array(count);
    MeshoptDecoder.decodeIndexSequence(new Uint8Array(out.buffer), count, 4, code);
    return out;
}

/** The mesh that a PackedMesh holds: the bytes of its vertices and its indices as the worker made them. */
function unpackMesh({ n, w, s, xy, v, r, ni, i }: PackedMesh): { vertices: ArrayBuffer; indices: Uint32Array<ArrayBuffer> } {
    const distinct = new Uint8Array(w * s);
    MeshoptDecoder.decodeVertexBuffer(distinct, w, s, v);
    const stride = xy ? s + 4 : s, wide = xy ? new Uint8Array(w * stride) : distinct;
    if (xy) {
        const from = new DataView(distinct.buffer), to = new DataView(wide.buffer);
        for (let k = 0; k < w; k++) {
            to.setInt32(k * stride, xy[0] + from.getInt16(k * s, true), true);
            to.setInt32(k * stride + 4, xy[1] + from.getInt16(k * s + 2, true), true);
            wide.set(distinct.subarray(k * s + 4, (k + 1) * s), k * stride + 8);
        }
    }
    let vertices = wide;
    if (r) {
        const of = sequence(n, r);
        vertices = new Uint8Array(n * stride);
        for (let k = 0; k < n; k++) vertices.set(wide.subarray(of[k] * stride, (of[k] + 1) * stride), k * stride);
    }
    return { vertices: vertices.buffer, indices: sequence(ni, i) };
}

/** The value of a baked node: decodeNode, with each packed mesh as the worker posts it. */
export async function unpackNode(bytes: Uint8Array): Promise<unknown> {
    await MeshoptDecoder.ready;
    const value = decodeNode(bytes);
    const walk = (item: unknown): unknown => {
        if (item === null || typeof item !== 'object' || ArrayBuffer.isView(item) || item instanceof ArrayBuffer) return item;
        if (Array.isArray(item)) return item.map(walk);
        const { $mesh, ...rest } = item as { $mesh?: PackedMesh };
        if ($mesh) return { ...unpackMesh($mesh), ...rest };
        return Object.fromEntries(Object.entries(item).map(([key, next]) => [key, walk(next)]));
    };
    return walk(value);
}
