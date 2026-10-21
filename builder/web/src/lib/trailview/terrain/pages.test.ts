import { describe, expect, it } from 'vitest';
import { worldPixel } from '../../planner/layers/mercator';
import { anchorAt, UNITS } from '../frame/space';
import { pageData, PAGE } from '../stream/heights';
import type { TerrainSource } from '../stream/terrain';
import { cdlod, COARSEST, key, MAP, RANGE, select, size, type Node } from './lod';
import { ARRIVALS, MODE, Pages, type StampSource, type Work } from './pages';

const BOX = [8.37, 46.765, 8.46, 46.83] as const;
const [west, north] = worldPixel([BOX[0], BOX[3]], UNITS), [east, south] = worldPixel([BOX[2], BOX[1]], UNITS);
const BOUNDS: [number, number, number, number] = [west, north, east, south];
const anchor = anchorAt(8.415, 46.7975);
const flat = () => pageData(new Float32Array(PAGE * PAGE).fill(1000), 1000, 1000, true, 'r32float');

/** Tiles of zooms 0 to 12; zoom 13 is absent. */
function source(): TerrainSource & { fetched: string[]; decoded: number } {
    const fetched: string[] = [];
    const tiles = {
        fetched, decoded: 0,
        fetch: async (z: number, x: number, y: number) => {
            fetched.push(`${z}/${x}/${y}`);
            return z <= 12 ? new ArrayBuffer(8) : undefined;
        },
        decode: async () => {
            tiles.decoded++;
            return [flat(), flat(), flat(), flat()];
        },
    };
    return tiles;
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
        const map = works.filter(work => work.map).at(-1)!.map!;
        const ready = [...pages.pages.values()].filter(page => page.ready);
        expect(map.filter(entry => entry !== 0).length).toBe(ready.length);
        // A stored page has its raw layer in the high half of its entry, for the syntheses.
        expect(map.filter(entry => entry >>> 16).length).toBe(ready.filter(page => page.kind === 'stored').length);
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

    it('decode each tile once, while its pages wait to arrive', async () => {
        const tiles = source(), pages = new Pages(tiles, { stored: 400, synth: 600 }, BOUNDS, () => {});
        await settle(pages, wanted);
        expect(tiles.decoded).toBe(tiles.fetched.filter(name => Number(name.split('/')[0]) <= 12).length);
    });

    it('count a shortage in the frame that wants too many pages, and evict none that it wants', async () => {
        const all = new Pages(source(), { stored: 400, synth: 600 }, BOUNDS, () => {});
        await settle(all, wanted);
        const synth = all.wanted.synth, pages = new Pages(source(), { stored: 400, synth }, BOUNDS, () => {});
        await settle(pages, wanted);
        // One more synthesized node, a child of a drawn one, before the finer ones in the list.
        const keys = new Set(wanted.map(key));
        const extra = wanted.filter(node => node.level === 16).map(node => ({ level: 17, x: 2 * node.x, y: 2 * node.y })).find(node => !keys.has(key(node)))!;
        const more = [...wanted, extra].sort((a, b) => a.level - b.level);
        const work = pages.update(more, anchor);
        expect(pages.shortage).toBe(1);
        expect(work.syntheses).toEqual([]);
        expect(wanted.every(node => pages.pages.get(key(node))?.ready)).toBe(true);
    });

    it('keep the stored pages that a synthesis reads when a page arrives in the same frame', async () => {
        const [w, n] = worldPixel([7.6, 47.25], UNITS), [e, s] = worldPixel([8.95, 46.46], UNITS);
        const at = (level: number) => ({ level, x: Math.floor((anchor.whole[0] + anchor.part[0]) / size(level)), y: Math.floor((anchor.whole[1] + anchor.part[1]) / size(level)) });
        const stored = at(13), around = [-1, 0, 1].flatMap(dy => [-1, 0, 1].map(dx => ({ level: 13, x: stored.x + dx, y: stored.y + dy })));
        const pages = new Pages(source(), { stored: 9, synth: 10 }, [w, n, e, s], () => {});
        // The pool holds the stored ancestor of the synthesis and its eight neighbours; zoom 13 is known absent.
        await settle(pages, [...around, at(14)]);
        expect(around.every(node => pages.pages.get(key(node))?.ready)).toBe(true);
        // A stored page far away is decoded and waits to arrive.
        const far = { level: 13, x: stored.x + 6, y: stored.y };
        pages.update([far], anchor);
        await new Promise(resolve => setTimeout(resolve));
        const work = pages.update([at(15), far], anchor);
        expect(work.syntheses.map(({ page, stored }) => [key(page.node), stored])).toEqual([[key(at(15)), 13]]);
        expect(around.every(node => pages.pages.get(key(node))?.ready)).toBe(true);
        expect(pages.shortage).toBe(1);
    });
});

describe('the stamps', () => {
    const { wanted } = select(cdlod({ anchor, above: 1.6 }, RANGE.least), BOUNDS, () => true);
    wanted.sort((a, b) => a.level - b.level);
    /** Every node has a feature near it; its stamps are known one frame after a page asks for them. */
    const stamps = (baked: boolean, water = false): StampSource => {
        const asked = new Set<string>();
        return { baked, get: node => (asked.has(key(node)) ? { data: new Uint32Array(8), heights: true, water } : void asked.add(key(node))) };
    };

    it('draw a stored page as a stamped copy, keep its raw layer for the syntheses, and stamp a synthesized page as it is made', async () => {
        const pages = new Pages(source(), { stored: 400, synth: 600 }, BOUNDS, () => {}, stamps(false));
        const works = await settle(pages, wanted);
        expect(pages.idle).toBe(true);
        const stored = [...pages.pages.values()].filter(page => page.kind === 'stored' && page.drawn !== undefined);
        expect(stored.length).toBeGreaterThan(0);
        expect(stored.every(page => page.copy !== undefined && page.drawn === page.copy)).toBe(true);
        const last = works.filter(work => work.map).at(-1)!;
        for (const { node: { level, x, y }, slot, copy } of stored) {
            const [ox, oy] = last.origins[level], entry = last.map![(level * MAP + y - oy) * MAP + x - ox];
            expect([entry & 0xffff, entry >>> 16]).toEqual([copy! + 1, slot + 1]);
        }
        for (const work of works) {
            work.syntheses.forEach(({ page }, k) => expect(work.stamps.find(stamp => stamp.page === page)).toMatchObject({ out: k, target: page.slot, mode: MODE.HEIGHTS }));
        }
    });

    it('with baked conditioning, stamp the stored pages in place before a synthesis reads them, and stamp no synthesized height', async () => {
        const pages = new Pages(source(), { stored: 400, synth: 600 }, BOUNDS, () => {}, stamps(true));
        const works = await settle(pages, wanted);
        expect(pages.idle).toBe(true);
        const stamped = new Map<string, number>(), frame = (work: Work) => works.indexOf(work);
        for (const work of works) for (const { page, target, mode } of work.stamps) {
            expect(page.kind === 'stored' ? [target, mode] : [mode & MODE.HEIGHTS]).toEqual(page.kind === 'stored' ? [page.slot, MODE.HEIGHTS | MODE.BAKED] : [0]);
            if (page.kind === 'stored') stamped.set(key(page.node), frame(work));
        }
        for (const work of works) for (const { page } of work.syntheses) {
            const ancestor = { level: 13, x: page.node.x >> (page.node.level - 13), y: page.node.y >> (page.node.level - 13) };
            expect(stamped.get(key(ancestor))).toBeLessThan(frame(work));
        }
    });

    it('count a shortage while a page with water has no water layer, and give it one when a layer frees', async () => {
        const pages = new Pages(source(), { stored: 400, synth: 600 }, BOUNDS, () => {}, stamps(false, true), { mask: 0, water: 8 });
        const all = await settle(pages, wanted);
        expect(Math.max(...all.map(work => work.stamps.length))).toBeLessThanOrEqual(ARRIVALS);
        const dry = () => [...pages.pages.values()].filter(page => page.dry && page.drawn !== undefined);
        expect(pages.shortage).toBeGreaterThan(0);
        expect(dry().length).toBeGreaterThan(0);
        // A view of fewer pages: the dry pages that it wants take the layers of the pages that it no longer wants.
        const fewer = dry().slice(0, 8).map(({ node }) => node);
        const works = await settle(pages, fewer, 2);
        expect(pages.shortage).toBe(0);
        expect(fewer.map(node => pages.pages.get(key(node))!).every(page => page.water !== undefined && !page.dry)).toBe(true);
        expect(works[0].stamps.filter(({ mode }) => mode === MODE.WATER)).toHaveLength(fewer.length);
    });
});

describe('a turn', () => {
    // A flight 1 km above the ground over Engelberg, in the whole box of the bakes.
    const [w, n] = worldPixel([7.6, 47.25], UNITS), [e, s] = worldPixel([8.95, 46.46], UNITS);
    const box: [number, number, number, number] = [w, n, e, s];
    const rule = cdlod({ anchor, above: 1000 }, RANGE.least);
    const ex = anchor.whole[0] + anchor.part[0];
    const side = (east: boolean) => select(rule, box, ({ level, x }) => (east ? (x + 1) * size(level) > ex : x * size(level) < ex));
    const ready = (pages: Pages, node: Node) => {
        for (let { level, x, y } = node; level >= COARSEST; level--, x >>= 1, y >>= 1) if (pages.pages.get(`${level}/${x}/${y}`)?.ready) return true;
        return false;
    };

    it('finds a ready page under every node of the new view before a tile of it arrives', async () => {
        const pages = new Pages(source(), { stored: 600, synth: 600 }, box, () => {});
        const east = side(true).wanted.sort((a, b) => a.level - b.level);
        await settle(pages, east);
        expect(pages.idle).toBe(true);
        const west = side(false).nodes;
        expect(west.filter(node => !ready(pages, node))).toEqual([]);
    });
});
