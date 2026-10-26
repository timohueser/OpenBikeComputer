import { describe, expect, it, vi } from 'vitest';
import type { GroundPage } from '../stream/ground-pages';
import { PAGE } from '../stream/heights';
import type { Node } from '../terrain/lod';
import { GroundPages, type GroundTiles, type GroundWork } from './pages';

const page = (): GroundPage => ({ cells: new Uint8Array(4 * PAGE * PAGE), colour: new Uint8Array(4 * PAGE * PAGE), snow: new Uint8Array(4 * PAGE * PAGE) });
/** The eye at the north-west corner of level-14 node (8580, 5770), near Engelberg. */
const EYE: [number, number] = [8.525390625, 46.890231573593994];
const row = (x0: number, n: number): Node[] => Array.from({ length: n }, (_, i) => ({ level: 14, x: x0 + i, y: 5770 }));

/**
 * Ground tiles that answer with four pages, coloured from level 14, except `failing`, which
 * fails its first load, and `absent`, which has no ground; the tiles asked for.
 */
function tiles(failing?: string, absent?: string): GroundTiles & { asked: string[] } {
    const asked: string[] = [];
    return {
        asked, coloured: 14,
        async pages(z, x, y) {
            const name = `${z}/${x}/${y}`;
            asked.push(name);
            if (name === failing && asked.filter(tile => tile === name).length === 1) throw new Error('Tile answered 500.');
            return { pages: name === absent ? [null, null, null, null] : [page(), page(), page(), page()], complete: true };
        },
    };
}

/** Frames of one view until the pages are idle. */
/** Frames until the pages are idle; the uploads of these frames. */
async function settle(pages: GroundPages, drawn: Node[], wanted = drawn, read: Node[] = [], frames = 50): Promise<GroundWork['uploads']> {
    const uploads: GroundWork['uploads'] = [];
    for (let i = 0; i < frames && (i === 0 || !pages.idle); i++) {
        uploads.push(...pages.update(drawn, wanted, [], [], EYE, read).uploads);
        await new Promise(resolve => setTimeout(resolve));
    }
    return uploads;
}

const resident = (pages: GroundPages) => [...pages.pages.values()].filter(slot => slot.date >= 0).map(slot => slot.node.x).sort((a, b) => a - b);

describe('the ground pages', () => {
    it('load a failed ground tile again after a wait, and keep an absent one out', async () => {
        vi.spyOn(console, 'warn').mockImplementation(() => {});
        const now = vi.spyOn(Date, 'now').mockReturnValue(0);
        const source = tiles('13/4290/2885', '13/4291/2885'), pages = new GroundPages(source, 32, 14, [0.5], 0, () => {});
        pages.setDate('2026-08-15');
        await settle(pages, row(8580, 4));
        expect(pages.idle).toBe(true);
        expect(resident(pages)).toEqual([]);
        now.mockReturnValue(2000);
        await settle(pages, row(8580, 4));
        expect(resident(pages)).toEqual([8580, 8581]);
        expect(source.asked.filter(tile => tile === '13/4290/2885')).toHaveLength(2);
        expect(source.asked.filter(tile => tile === '13/4291/2885')).toHaveLength(1);
        now.mockRestore();
    });

    it('give a full pool to the nearest drawn pages, and take slots from wanted pages that are farther', async () => {
        const pages = new GroundPages(tiles(), 4, 14, [0.5], 0, () => {});
        pages.setDate('2026-08-15');
        await settle(pages, row(8600, 4));
        expect(resident(pages)).toEqual([8600, 8601, 8602, 8603]);
        // The view moves to the eye: the old pages are still wanted, as cover, but the drawn ones near the eye are first.
        await settle(pages, row(8578, 6), [...row(8578, 6), ...row(8600, 4)]);
        expect(resident(pages)).toEqual([8578, 8579, 8580, 8581]);
        expect(pages.shortage).toBe(6);
    });

    it('keep the pages that the instances read, where the terrain draws finer ones', async () => {
        const source = tiles(), pages = new GroundPages(source, 32, 14, [0.5], 0, () => {});
        pages.setDate('2026-08-15');
        await settle(pages, row(8580, 4), row(8580, 4), [{ level: 10, x: 536, y: 360 }]);
        expect(source.asked[0]).toBe('9/268/180');
        expect(pages.pages.get('10/536/360')?.date).toBe(pages.date);
    });

    it('keep the openness refreshes that a frame has no room for', async () => {
        const pages = new GroundPages(tiles(), 32, 14, [0.5], 0, () => {});
        pages.setDate('2026-08-15');
        const drawn = row(8570, 20);
        await settle(pages, drawn);
        // The heights that arrived also march the horizons of the 20 pages, two a frame: 10 frames.
        const counts = [pages.update(drawn, drawn, drawn, [], EYE), ...Array.from({ length: 9 }, () => pages.update(drawn, drawn, [], [], EYE))].map(work => work.refreshes.length);
        expect(counts).toEqual([8, 8, 4, 0, 0, 0, 0, 0, 0, 0]);
        expect(pages.idle).toBe(true);
    });

    it('ask the terrain for the coarse node that the relief reads, and build the relief again when it or a neighbour changes', async () => {
        const pages = new GroundPages(tiles(), 32, 14, [0.5], 0, () => {});
        pages.setDate('2026-08-15');
        const drawn = row(8580, 4);
        await settle(pages, drawn);
        expect(pages.reads).toEqual(expect.arrayContaining([{ level: 13, x: 4290, y: 2885 }, { level: 13, x: 4291, y: 2885 }]));
        const refreshes = (arrived: Node) => pages.update(drawn, drawn, [arrived], [], EYE).refreshes.map(slot => slot.node.x).sort();
        // Pages 8580 and 8581 read coarse node 4290, 8582 and 8583 its neighbour 4291.
        expect(refreshes({ level: 13, x: 4289, y: 2885 })).toEqual([8580, 8581]);
        expect(refreshes({ level: 13, x: 4292, y: 2884 })).toEqual([8582, 8583]);
        expect(refreshes({ level: 14, x: 8579, y: 5771 })).toEqual([8580]);
    });

    it('march the horizons again where heights arrive within the reach of the march, and keep them over a new date', async () => {
        const pages = new GroundPages(tiles(), 32, 14, [0.5], 0, () => {});
        pages.setDate('2026-08-15');
        const drawn = row(8580, 4);
        expect((await settle(pages, drawn)).map(upload => upload.horizons)).toEqual([true, true, true, true]);
        // The march of page 8583 reads 21 texels into page 8584 and 42 into node 4292 of level 13; that of page 8582 reads neither.
        const arrived = [{ level: 14, x: 8584, y: 5770 }, { level: 13, x: 4292, y: 2885 }];
        expect(pages.update(drawn, drawn, arrived, [], EYE).horizons.map(slot => slot.node.x)).toEqual([8583]);
        pages.setDate('2026-08-16');
        expect((await settle(pages, drawn)).map(upload => upload.horizons)).toEqual([false, false, false, false]);
    });
});
