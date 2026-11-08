import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { decodeNode, encodeNode, unpackNode } from './node-bytes';
import { OWNER, type Node } from './lod';

const body = ({ level, x, y }: Node) => {
    const bytes = readFileSync(new URL(`../../../../tests/browser/trailview/fixture/features/${level}/${x}/${y}.bin`, import.meta.url));
    return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
};

describe('node bytes', () => {
    it('give back what the stamp worker posts for the coarse nodes and the owner nodes of a tile', async () => {
        // The stamp worker's global.
        Object.assign(globalThis, { self: globalThis });
        const { bakeTile, stamps } = await import('./stamp-bake');
        for (const tile of [{ level: 11, x: 1071, y: 721 }, { level: 13, x: 4288, y: 2887 }]) {
            const baked = await bakeTile(tile, body(tile)), held = baked.filter(([node]) => node.level !== tile.level);
            if (tile.level === 13) {
                const mask = baked.find(([node]) => node.level === tile.level)![1];
                expect(mask[0] | (mask[1] << 8)).toBe(held.reduce((bits, [node]) => bits | (1 << ((node.x & 3) + 4 * (node.y & 3))), 0));
            }
            expect(held.length).toBeGreaterThan(1);
            for (const [node, bytes] of held) {
                const posted = await stamps(tile, node, body(tile)), value = node.level === OWNER ? posted.meshes : posted;
                expect(await unpackNode(bytes)).toStrictEqual(value);
                expect(encodeNode(await unpackNode(bytes))).toStrictEqual(encodeNode(value));
            }
        }
    }, 30_000);

    it('refuse a value that JSON changes', () => {
        for (const value of [{ a: NaN }, [-0], { a: Infinity }, { a: new Map() }, { a: new DataView(new ArrayBuffer(4)) }]) expect(() => encodeNode(value)).toThrow();
        expect(decodeNode(encodeNode({ a: [1.5, null, true], b: new Float64Array([0.1, -2]), c: new ArrayBuffer(3) }))).toStrictEqual({ a: [1.5, null, true], b: new Float64Array([0.1, -2]), c: new ArrayBuffer(3) });
    });
});
