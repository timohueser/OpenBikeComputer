import { describe, expect, it } from 'vitest';
import { anchorAt } from '../frame/space';
import { TILE_ZOOM } from '../stream/terrain';
import { cdlod, GRID, RANGE, select, size } from './lod';
import { MODE, type Page } from './pages';
import { gridIndices, stampJobs, SYNTH_PAGES, SYNTHESIZED, Terrain } from './terrain';

describe('the synthesized pool', () => {
    it('holds what the least range wants of the half of the world ahead, so no shortage takes away the finest level', () => {
        const anchor = anchorAt(8.38, 46.8), east = anchor.whole[0] + anchor.part[0];
        for (const above of [1.6, 30, 300]) {
            const { wanted } = select(cdlod({ anchor, above }, RANGE.least), [0, 0, 2 ** 30, 2 ** 30], ({ level, x }) => (x + 1) * size(level) >= east);
            expect(wanted.filter(({ level }) => level > TILE_ZOOM + 1).length).toBeLessThanOrEqual(SYNTH_PAGES);
        }
    });
});

describe('the stamp jobs', () => {
    const page = (kind: Page['kind'], slot: number, drawn: number): Page => ({ node: { level: 18, x: 1, y: 2 }, kind, slot, used: 0, range: [0, 0], complete: true, ready: true, drawn, mask: 3 });

    it('read the heights of a page synthesized in the frame from `out`, a stored page\'s from its raw layer, and a mask alone from the drawn layer', () => {
        const fresh = page('synth', 7, 7), old = page('synth', 8, 8), stored = page('stored', 2, 40);
        const stamps = [
            { page: fresh, out: 0, target: 7, data: null, mode: MODE.MASK },
            { page: old, out: 0, target: 8, data: null, mode: MODE.MASK },
            { page: stored, out: 1, target: 40, data: null, mode: MODE.HEIGHTS },
            { page: fresh, out: 0, target: 7, data: null, mode: MODE.HEIGHTS | MODE.MASK },
        ];
        const words = new Uint32Array(stampJobs(stamps, new Set([fresh])).jobs), step = words.length / stamps.length;
        // `base`, the sixth word of a StampJob.
        expect(stamps.map((_, k) => words[step * k + 5])).toEqual([SYNTHESIZED, 8, 2, SYNTHESIZED]);
    });
});

describe('the paint of the near field', () => {
    const node = { level: 18, x: 1, y: 2 }, name = '18/1/2', anchor = { whole: [0, 0], part: [0, 0], unit: 1 };
    const painted = (stamps: { data: Uint32Array | null; heights: boolean; mask: boolean }) => (Terrain.prototype as unknown as { painted: (nodes: unknown[], near: Set<string>, at: unknown) => boolean }).painted
        .call({ pages: { pages: new Map([[name, { drawn: 0 }]]), maskReach: Infinity }, stamps: { known: () => stamps } }, [node], new Set([name]), anchor);

    it('waits for the mask of a node that holds one, and for none of a node that stamps only heights', () => {
        expect(painted({ data: new Uint32Array(352), heights: true, mask: true })).toBe(false);
        expect(painted({ data: new Uint32Array(352), heights: true, mask: false })).toBe(true);
    });
});

describe('the frames of a still view', () => {
    const working = (quality: number) => Object.assign(Object.create(Terrain.prototype), {
        pages: { idle: true, loads: 0, capacity: { stored: 100, synth: 100 }, wanted: { stored: 10, synth: 10 } }, stamps: { loading: false }, newContent: false, calm: 1, quality,
    }).working;

    it('go on while the quality can rise, and end once it is at its best', () => {
        expect(working(1)).toBe(true);
        expect(working(0)).toBe(false);
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
