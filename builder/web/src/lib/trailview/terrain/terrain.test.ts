import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { GRID } from './lod';
import { EXACT_M, gridIndices } from './terrain';

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
    it('takes the data of the nodes as far from the eye as roads.wgsl paints from it', () => {
        const roads = readFileSync(new URL('../ground/roads.wgsl', import.meta.url), 'utf8');
        expect(Number(/const EXACT_M: f32 = ([\d.]+);/.exec(roads)?.[1])).toBe(EXACT_M);
    });
});
