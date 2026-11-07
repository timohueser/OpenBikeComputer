import { describe, expect, it } from 'vitest';
import { CALM } from '../terrain/lod';
import type { CellMesh } from './mesh';
import { meshLevel, splitMouths } from './pieces';

describe('splitMouths', () => {
    it("moves a run's mouth triangles to its end and keeps every triangle with its first corner", () => {
        // The corners' labels in their words, as the mesh packs them: 13 bits in the first word, 12 in the second.
        const labels = [0, 1, 2, 3, 8193, 1, 2, 3];
        const corners = new Uint32Array(labels.flatMap(l => [((l & 0x1fff) << 19) >>> 0, ((l >>> 13) << 19) >>> 0]));
        // Two runs; the second's indices add its base, 4.
        const runs = [[[0, 1, 2], [1, 2, 3], [3, 0, 1], [2, 3, 0]], [[0, 1, 2], [3, 2, 1], [1, 0, 3]]];
        const mesh: CellMesh = {
            x: 0,
            y: 0,
            corners,
            triangles: new Uint16Array(runs.flat(2)),
            runs: [{ count: 4, base: 0 }, { count: 3, base: 4 }],
            level18Runs: 0,
            steps: new Uint32Array(0),
            sunk: new Uint32Array(0),
        };
        // Label 8193 has a bit in the second word.
        expect(splitMouths(mesh, new Set([1, 8193]))).toEqual([1, 2]);
        const tris = Array.from({ length: mesh.triangles.length / 3 }, (_, t) => Array.from(mesh.triangles.subarray(3 * t, 3 * t + 3)));
        expect(tris.slice(0, 4)).toEqual([[0, 1, 2], [3, 0, 1], [2, 3, 0], [1, 2, 3]]);
        expect(tris.slice(4)).toEqual([[3, 2, 1], [0, 1, 2], [1, 0, 3]]);
    });
});

describe('meshLevel', () => {
    /** The mesh levels of frames at these finest levels, from a calm start. */
    const levels = (finest: number[]) => {
        let calm = CALM;
        return finest.map(f => {
            const next = meshLevel(f, calm);
            calm = next.calm;
            return next.level;
        });
    };

    it('draws over level 18, or 17 at a lower quality, and stops at once below 17', () => {
        expect(levels([18, 17, 18, 16, 15])).toEqual([18, 17, 18, 0, 0]);
    });

    it('draws again only after CALM frames in a row at level 17 or finer', () => {
        const back = levels([16, ...Array<number>(CALM).fill(17)]);
        expect(back.slice(0, CALM)).toEqual(Array(CALM).fill(0));
        expect(back[CALM]).toBe(17);
        // A dip to 16 starts the count again.
        expect(levels([16, ...Array<number>(CALM - 1).fill(18), 16, 18]).every(level => level === 0)).toBe(true);
    });
});
