import { describe, expect, it, vi } from 'vitest';
import { anchorAt } from '../frame/space';
import { TILE_ZOOM } from '../stream/terrain';
import { CALM, cdlod, GRID, RANGE, select, size } from './lod';
import { MODE, type Page } from './pages';
import { gridIndices, stampJobs, stepQuality, SYNTH_PAGES, SYNTHESIZED, Terrain } from './terrain';

describe('the synthesized pool', () => {
    it('holds what the least range wants of the half of the world ahead, so no page is left out', () => {
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

describe('the quality', () => {
    /** The qualities of the frames of a still view whose pages coarser than the meshes' need demand[q] slots at quality q, in a pool of `capacity`. */
    const still = (demand: number[], capacity: number, from: number) => {
        let state = { quality: from, calm: 0, held: false };
        return Array.from({ length: 3 * CALM }, () => {
            const q = state.quality;
            state = stepQuality(state, demand[q] > capacity, true, false, () => q + 1 < demand.length && demand[q + 1] < demand[q], () => demand[q - 1] <= capacity);
            return state.quality;
        });
    };

    it('comes to a fixed quality for fixed pages and pools, and steps only where a step lowers the demand', () => {
        // A usual screen: no step makes the range smaller, so the pages left out alone answer the shortage.
        expect(still([90], 70, 0).every(q => q === 0)).toBe(true);
        // A large screen: from either side, the finest quality whose demand fits, which then stays.
        const demand = [120, 100, 80, 65, 60];
        for (const from of [0, 2, 4]) expect(still(demand, 70, from).slice(5).every(q => q === 3)).toBe(true);
        // A pool that no quality fits: the coarsest.
        expect(still(demand, 50, 0).slice(5).every(q => q === 4)).toBe(true);
    });
});

describe('the frames of a still view', () => {
    const working = (calm: number) => Object.assign(Object.create(Terrain.prototype), {
        pages: { idle: true, loads: 0 }, stamps: { loading: false }, newContent: false, calm,
    }).working;

    it('go on for the frame after a quality step, in which a finer quality is tried, and then end', () => {
        expect(working(0)).toBe(true);
        expect(working(1)).toBe(false);
    });
});

describe('the attach to a device', () => {
    // Stops the attach at its first use of the device: what it does to the records comes before.
    const recordsLost = async (resources: object | undefined) => {
        const lost = vi.fn(), gpu = { get device(): never { throw new Error('no device'); } };
        await expect(Terrain.prototype.attach.call({ resources, records: { lost }, pages: { lost() {} } } as never, gpu as never, {} as never)).rejects.toThrow('no device');
        return lost.mock.calls.length;
    };

    it('keeps the records built before the first device, and drops those that a lost device held', async () => {
        expect(await recordsLost(undefined)).toBe(0);
        expect(await recordsLost({})).toBe(1);
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
