import { describe, expect, it, vi } from 'vitest';
import type { GroundPage } from '../stream/ground-pages';
import { PAGE } from '../stream/heights';
import { key, type Node } from '../terrain/lod';
import { GroundPages, type GroundTiles } from './pages';

const page = (): GroundPage => ({ cells: new Uint8Array(4 * PAGE * PAGE), colour: new Uint8Array(4 * PAGE * PAGE), snow: new Uint8Array(4 * PAGE * PAGE) });
/** The eye at the north-west corner of level-14 node (8580, 5770), near Engelberg. */
const EYE: [number, number] = [8.525390625, 46.890231573593994];
const row = (x0: number, n: number): Node[] => Array.from({ length: n }, (_, i) => ({ level: 14, x: x0 + i, y: 5770 }));

/**
 * Material tiles that answer with four pages, coloured from level `coloured`, except `failing`, which
 * fails its first load, and `absent`, which has no ground; the tiles asked for.
 */
function tiles(failing?: string, coloured = 14, absent?: string): GroundTiles & { asked: string[] } {
    const asked: string[] = [];
    return {
        asked, coloured,
        async pages(z, x, y) {
            const name = `${z}/${x}/${y}`;
            asked.push(name);
            if (name === failing && asked.filter(tile => tile === name).length === 1) throw new Error('Tile answered 500.');
            return { pages: name === absent ? [null, null, null, null] : [page(), page(), page(), page()], complete: true };
        },
    };
}

/** Frames of one view until the pages are idle. */
async function settle(pages: GroundPages, drawn: Node[], wanted = drawn, frames = 50): Promise<void> {
    for (let i = 0; i < frames && (i === 0 || !pages.idle); i++) {
        pages.update(drawn, wanted, [], [], EYE);
        await new Promise(resolve => setTimeout(resolve));
    }
}

const resident = (pages: GroundPages) => [...pages.pages.values()].filter(slot => slot.date >= 0).map(slot => slot.node.x).sort((a, b) => a - b);

describe('the ground pages', () => {
    it('load a failed material tile again after a wait, and keep an absent one out', async () => {
        vi.spyOn(console, 'warn').mockImplementation(() => {});
        const now = vi.spyOn(Date, 'now').mockReturnValue(0);
        const source = tiles('13/4290/2885', 14, '13/4291/2885'), pages = new GroundPages(source, 32, 14, [0.5], 0, () => {});
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

    it('keep the coloured cover of the drawn pages first, and draw a replaced page until its replacement arrives', async () => {
        const pages = new GroundPages(tiles(undefined, 12), 4, 14, [0.5], 0, () => {});
        pages.setDate('2026-08-15');
        await settle(pages, row(8578, 6));
        const keys = () => [...pages.pages.values()].filter(slot => slot.date >= 0).map(slot => key(slot.node)).sort();
        expect(keys()).toEqual(['12/2144/1442', '12/2145/1442', '14/8579/5770', '14/8580/5770']);
        pages.update(row(8574, 6), row(8574, 6), [], [], EYE);
        expect(keys()).toEqual(['12/2144/1442', '12/2145/1442', '14/8579/5770', '14/8580/5770']);
    });

    it('keep the openness refreshes that a frame has no room for', async () => {
        const pages = new GroundPages(tiles(), 32, 14, [0.5], 0, () => {});
        pages.setDate('2026-08-15');
        const drawn = row(8570, 20);
        await settle(pages, drawn);
        const counts = [pages.update(drawn, drawn, drawn, [], EYE), pages.update(drawn, drawn, [], [], EYE), pages.update(drawn, drawn, [], [], EYE)].map(work => work.refreshes.length);
        expect(counts).toEqual([8, 8, 4]);
        expect(pages.idle).toBe(true);
    });
});
