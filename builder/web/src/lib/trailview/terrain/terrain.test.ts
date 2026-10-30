import { describe, expect, it } from 'vitest';
import { GRID } from './lod';
import { gridIndices, Terrain } from './terrain';

describe('the paint of the near field', () => {
    const node = { level: 18, x: 1, y: 2 }, name = '18/1/2', anchor = { whole: [0, 0], part: [0, 0], unit: 1 };
    const painted = (stamps: { data: Uint32Array | null; heights: boolean; mask: boolean }) => (Terrain.prototype as unknown as { painted: (nodes: unknown[], near: Set<string>, at: unknown) => boolean }).painted
        .call({ pages: { pages: new Map([[name, { drawn: 0 }]]), maskReach: Infinity }, stamps: { known: () => stamps } }, [node], new Set([name]), anchor);

    it('waits for the mask of a node that holds one, and for none of a node that stamps only heights', () => {
        expect(painted({ data: new Uint32Array(352), heights: true, mask: true })).toBe(false);
        expect(painted({ data: new Uint32Array(352), heights: true, mask: false })).toBe(true);
    });
});

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
