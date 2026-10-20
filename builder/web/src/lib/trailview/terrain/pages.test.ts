import { describe, expect, it } from 'vitest';
import { worldPixel } from '../../planner/layers/mercator';
import { anchorAt, UNITS } from '../frame/space';
import { pageData, PAGE } from '../stream/heights';
import type { TerrainSource } from '../stream/terrain';
import { cdlod, key, RANGE, select, type Node } from './lod';
import { Pages, STORED_BIT, type Work } from './pages';

const BOX = [8.37, 46.765, 8.46, 46.83] as const;
const [west, north] = worldPixel([BOX[0], BOX[3]], UNITS), [east, south] = worldPixel([BOX[2], BOX[1]], UNITS);
const BOUNDS: [number, number, number, number] = [west, north, east, south];
const anchor = anchorAt(8.415, 46.7975);
const flat = () => pageData(new Float32Array(PAGE * PAGE).fill(1000), 1000, 1000, 'r32float');

/** Tiles of zooms 0 to 12, each fetched once; zoom 13 is absent. */
function source(): TerrainSource & { fetched: string[] } {
    const fetched: string[] = [];
    return {
        fetched,
        fetch: async (z, x, y) => {
            fetched.push(`${z}/${x}/${y}`);
            return z <= 12 ? new ArrayBuffer(8) : undefined;
        },
        decode: async () => [flat(), flat(), flat(), flat()],
    };
}

/** Frames of the walk view until the pages are idle; the work of each frame. */
async function settle(pages: Pages, wanted: Node[], frames = 300): Promise<Work[]> {
    const works: Work[] = [];
    for (let i = 0; i < frames && (i === 0 || !pages.idle); i++) {
        works.push(pages.update(wanted, anchor));
        await new Promise(resolve => setTimeout(resolve));
    }
    return works;
}

describe('the page pools', () => {
    const { wanted } = select(cdlod({ anchor, above: 1.6 }, RANGE.least), BOUNDS, () => true);
    wanted.sort((a, b) => a.level - b.level);

    it('synthesize the levels below the stored tiles from the deepest stored page, and map every ready page', async () => {
        const pages = new Pages(source(), { stored: 400, synth: 600 }, BOUNDS, () => {});
        const works = await settle(pages, wanted);
        expect(pages.idle).toBe(true);
        const syntheses = works.flatMap(work => work.syntheses);
        expect(syntheses.length).toBeGreaterThan(0);
        // Zoom 12 holds the pages of level 13, the deepest stored level.
        expect(syntheses.every(({ page, stored }) => page.node.level > 13 && stored === 13)).toBe(true);
        const map = works.findLast(work => work.map)!.map!;
        const ready = [...pages.pages.values()].filter(page => page.ready);
        expect(map.filter(entry => entry !== 0).length).toBe(ready.length);
        expect(map.filter(entry => entry & STORED_BIT).length).toBe(ready.filter(page => page.kind === 'stored').length);
    });

    it('stay at their size, and count a shortage when a view wants more pages than a pool holds', async () => {
        const pages = new Pages(source(), { stored: 400, synth: 20 }, BOUNDS, () => {});
        await settle(pages, wanted, 60);
        const resident = [...pages.pages.values()];
        expect(resident.filter(page => page.kind === 'synth').length).toBeLessThanOrEqual(20);
        expect(new Set(resident.map(page => page.slot)).size).toBe(resident.length);
        expect(pages.shortage).toBeGreaterThan(0);
        expect(pages.wanted.synth).toBeGreaterThan(20);
    });

    it('get every page again for a new device, without a request', async () => {
        const tiles = source(), pages = new Pages(tiles, { stored: 400, synth: 600 }, BOUNDS, () => {});
        await settle(pages, wanted);
        const before = new Set([...pages.pages.keys()]), requests = tiles.fetched.length;
        pages.lost();
        await settle(pages, wanted);
        expect(new Set([...pages.pages.keys()])).toEqual(before);
        expect(tiles.fetched.length).toBe(requests);
        expect(new Set(tiles.fetched).size).toBe(requests);
        expect([...before].every(name => pages.pages.get(name)?.ready)).toBe(true);
        expect(wanted.some(node => !before.has(key(node)))).toBe(false);
    });
});
