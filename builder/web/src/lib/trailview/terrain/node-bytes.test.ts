import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { decodeNode, encodeNode } from './node-bytes';
import type { Node } from './lod';

const body = ({ level, x, y }: Node) => {
    const bytes = readFileSync(new URL(`../../../../tests/browser/trailview/fixture/features/${level}/${x}/${y}.bin`, import.meta.url));
    return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
};

describe('node bytes', () => {
    it('give back what the stamp worker posts for a coarse node and an owner node', async () => {
        // The stamp worker's global.
        Object.assign(globalThis, { self: globalThis });
        const { stamps } = await import('./stamp-bake');
        const coarse = { level: 11, x: 1071, y: 721 }, owner = { level: 13, x: 4287, y: 2887 };
        for (const [tile, node] of [[coarse, { level: 12, x: 2143, y: 1443 }], [owner, { level: 15, x: 17148, y: 11551 }]]) {
            const posted = await stamps(tile, node, body(tile)), bytes = encodeNode(posted);
            expect(decodeNode(bytes)).toStrictEqual(posted);
            expect(encodeNode(decodeNode(bytes))).toStrictEqual(bytes);
        }
    });

    it('refuse a value that JSON changes', () => {
        for (const value of [{ a: NaN }, [-0], { a: Infinity }, { a: new Map() }, { a: new DataView(new ArrayBuffer(4)) }]) expect(() => encodeNode(value)).toThrow();
        expect(decodeNode(encodeNode({ a: [1.5, null, true], b: new Float64Array([0.1, -2]), c: new ArrayBuffer(3) }))).toStrictEqual({ a: [1.5, null, true], b: new Float64Array([0.1, -2]), c: new ArrayBuffer(3) });
    });
});
