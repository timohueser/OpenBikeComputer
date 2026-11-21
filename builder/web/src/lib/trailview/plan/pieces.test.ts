import { describe, expect, it } from 'vitest';
import { anchorAt } from '../frame/space';
import { CALM, cdlod, RANGE } from '../terrain/lod';
import type { CellMesh } from './mesh';
import { meshLevel, Pieces, splitMouths, splitTails } from './pieces';

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
            endFaces: 0,
            sunk: new Uint32Array(0),
            culverts: new Uint32Array(0),
        };
        // Label 8193 has a bit in the second word.
        expect(splitMouths(mesh, new Set([1, 8193]))).toEqual([1, 2]);
        const tris = Array.from({ length: mesh.triangles.length / 3 }, (_, t) => Array.from(mesh.triangles.subarray(3 * t, 3 * t + 3)));
        expect(tris.slice(0, 4)).toEqual([[0, 1, 2], [3, 0, 1], [2, 3, 0], [1, 2, 3]]);
        expect(tris.slice(4)).toEqual([[3, 2, 1], [0, 1, 2], [1, 0, 3]]);
    });
});

describe('splitTails', () => {
    it('moves the triangles and slots whose plan meets a grown box to the end of their group, in their order', () => {
        // Corners on a grid of 300 world units (about 11 m at the equator), from the cell's corner; label 0.
        const grid = [0, 300, 600, 900, 1200].flatMap(x => [0, 300, 600].map(y => [x, y]));
        const at = (x: number, y: number) => grid.findIndex(p => p[0] === x && p[1] === y);
        const corners = new Uint32Array(grid.flatMap(([x, y]) => [16 * x, 16 * y]));
        // A box 600 units east and 300 south of the corner, heading east: 5 m deep, 3 m to each side. Grown by a grid
        // step it reaches about 536 to 798 east and 155 to 445 south.
        const box = new Float64Array([600, 2 ** 29 + 300, 1, 0, 5, 3, 5, 0, 0, 5, 0, 0]);
        const inside = [at(300, 300), at(600, 300), at(600, 600)], across = [at(300, 0), at(900, 0), at(600, 600)];
        const far = [at(0, 0), at(300, 0), at(0, 300)], west = [at(0, 300), at(300, 600), at(0, 600)], beside = [at(900, 0), at(1200, 0), at(1200, 300)];
        // The second run's indices add its base, 1; its last two triangles are its mouth's.
        const runs = [[inside, far, across], [west, beside, inside].map(t => t.map(k => k - 1))];
        const mesh: CellMesh = {
            x: 0, y: 0, corners, triangles: new Uint16Array(runs.flat(2)), runs: [{ count: 3, base: 0 }, { count: 3, base: 1 }], level18Runs: 0,
            // Two steps and an end face: a slot stands from its first corner to its second.
            steps: new Uint32Array([[at(600, 0), at(600, 600)], [at(0, 0), at(300, 0)], [at(300, 300), at(600, 300)]].flatMap(([p, q]) => [p, q, q, p])),
            endFaces: 1, sunk: new Uint32Array(0), culverts: new Uint32Array(0),
        };
        // A box 3 km east: the cell does not meet it, and its tail tests only the first.
        const away = box.map((v, i) => (i === 0 ? v + 80_000 : v));
        const tails = splitTails(mesh, [0, 2], [0, 2 ** 29], [box, away]);
        expect(tails).toEqual({ own: [2, 0], mouth: [0, 1], slots: [1, 1], boxes: [box] });
        const tris = Array.from({ length: 6 }, (_, t) => Array.from(mesh.triangles.subarray(3 * t, 3 * t + 3)).map(k => k + (t < 3 ? 0 : 1)));
        expect(tris).toEqual([far, inside, across, west, inside, beside]);
        expect(Array.from(mesh.steps.filter((_, i) => i % 4 === 0))).toEqual([at(0, 0), at(600, 0), at(300, 300)]);
        // No box: nothing moves.
        expect(splitTails(mesh, [0, 2], [0, 2 ** 29], [])).toEqual({ own: [0, 0], mouth: [0, 0], slots: [0, 0], boxes: [] });
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

describe('Pieces', () => {
    it('draws again after a dip to 16 in a still view: it asks for the frames that bring the meshes back', () => {
        const pieces = new Pieces(() => ({ data: null, portals: null })), eye = { anchor: anchorAt(8.4048, 46.8219), above: 1.7 };
        const frame = (finest: number) => pieces.update(cdlod(eye, RANGE.least, finest), eye, () => 0, () => '', [], () => true, () => true);
        for (const finest of [18, 17, 16]) frame(finest);
        expect(pieces.level).toBe(0);
        // The quality is back at 17 and nothing else moves: only the meshes keep the frames going.
        let frames = 1;
        for (frame(17); pieces.returning && frames < 10 * CALM; frames++) frame(17);
        expect(pieces.level).toBe(17);
        expect(frames).toBe(CALM);
    });

    it('reads its pages again after a rest below 17, as pages came and went while it rested', () => {
        const pieces = new Pieces(() => ({ data: null, portals: null })), eye = { anchor: anchorAt(8.4048, 46.8219), above: 1.7 };
        // A cell's mesh with no corners: the head (MAGIC, HEAD 12 words) and the region words.
        const words = new Uint32Array(12 + Math.ceil((130 * 130) / 32));
        words[0] = 0x4f424d05;
        pieces.add({ level: 16, x: 34_307, y: 23_107 }, words.buffer, 0, new Set());
        let reads = 0;
        const frame = (finest: number) => pieces.update(cdlod(eye, RANGE.least, finest), eye, () => 0, () => `${reads++}`, [], () => true, () => true);
        frame(18);
        frame(16);
        const before = reads;
        // No page is touched in this frame: only the rest makes the piece read its keys.
        frame(17);
        expect(reads).toBeGreaterThan(before);
    });
});
