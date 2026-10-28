import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { GRID } from './lod';
import { EXACT_M, gridIndices, packNear } from './terrain';

describe('gridIndices', () => {
    it('draws each triangle of drawn_height once, with its winding', () => {
        const n = GRID + 1, cells: string[] = [];
        for (let j = 0; j < GRID; j++) for (let i = 0; i < GRID; i++) {
            const a = j * n + i;
            cells.push(`${a},${a + n},${a + 1}`, `${a + 1},${a + n},${a + n + 1}`);
        }
        const indices = gridIndices(), triangles: string[] = [];
        for (let k = 0; k < indices.length; k += 3) triangles.push(indices.slice(k, k + 3).join(','));
        expect(triangles.sort()).toEqual(cells.sort());
    });
});

describe('the exact paint', () => {
    const roads = readFileSync(new URL('../ground/roads.wgsl', import.meta.url), 'utf8');

    it('takes the data of the nodes as far from the eye as roads.wgsl paints from it', () => {
        expect(Number(/const EXACT_M: f32 = ([\d.]+);/.exec(roads)?.[1])).toBe(EXACT_M);
    });

    it('packs the nodes as exact_mask of roads.wgsl finds them, and leaves out a node that does not fit', () => {
        // The words that exact_mask reads of entry i: x and y of the first texel at `node`, then the level and the offset.
        expect(roads).toMatch(/let node = 1u \+ 4u \* i;\s+let level = stamp_data\[node \+ 2u\];\s+let first = vec2i\(i32\(stamp_data\[node\]\), i32\(stamp_data\[node \+ 1u\]\)\);/);
        expect(roads).toMatch(/paint_at\(level, first, stamp_data\[node \+ 3u\], p, f\)/);
        const inside = (node: { x: number; y: number }) => [node.x * 4096 + 1000, node.y * 4096 + 3000];
        // As exact_mask: the offset of the data of the node of level 18 that holds a world point.
        const find = (packed: Uint32Array, [px, py]: number[]) => {
            for (let i = 0; i < packed[0]; i++) {
                const node = 1 + 4 * i, level = packed[node + 2];
                if (px >> (30 - level) === packed[node] >> 8 && py >> (30 - level) === packed[node + 1] >> 8) return packed[node + 3];
            }
            return undefined;
        };
        const big = { node: { level: 18, x: 137190, y: 92394 }, data: new Uint32Array(100).fill(7) };
        const near = [big, { node: { level: 18, x: 137190, y: 92395 }, data: Uint32Array.of(1, 2, 3) }, { node: { level: 18, x: 137191, y: 92395 }, data: Uint32Array.of(4, 5) }];
        for (const [words, kept] of [[1 << 20, near], [80, near.slice(1)]] as const) {
            const packed = packNear(near, words);
            expect(packed[0]).toBe(kept.length);
            for (const { node, data } of near) {
                const at = find(packed, inside(node));
                expect(at === undefined ? undefined : Array.from(packed.subarray(at, at + data.length))).toEqual(kept.some(entry => entry.node === node) ? Array.from(data) : undefined);
            }
        }
    });
});
