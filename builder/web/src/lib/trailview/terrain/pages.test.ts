import { describe, expect, it } from 'vitest';
import { worldPixel } from '../../planner/layers/mercator';
import { anchorAt, UNITS } from '../frame/space';
import { pageData, PAGE } from '../stream/heights';
import type { TerrainSource } from '../stream/terrain';
import { cdlod, distances, EYE_M, key, MAP, RANGE, select, size, type Node } from './lod';
import { ARRIVALS, MODE, Pages, type StampSource, type Work } from './pages';

const BOX = [8.37, 46.765, 8.46, 46.83] as const;
const [west, north] = worldPixel([BOX[0], BOX[3]], UNITS), [east, south] = worldPixel([BOX[2], BOX[1]], UNITS);
const BOUNDS: [number, number, number, number] = [west, north, east, south];
const anchor = anchorAt(8.415, 46.7975);
// One page for every decode: coding a page takes milliseconds, and Pages never writes to one.
const FLAT = pageData(new Float32Array(PAGE * PAGE).fill(1000), 1000, 1000, true);

/** Tiles of zooms 0 to `deepest`; the next zoom is absent. */
function source(deepest = 12): TerrainSource & { fetched: string[]; decoded: number } {
    const fetched: string[] = [];
    const tiles = {
        fetched, decoded: 0,
        fetch: async (z: number, x: number, y: number) => {
            fetched.push(`${z}/${x}/${y}`);
            return z <= deepest ? new ArrayBuffer(8) : undefined;
        },
        decode: async () => {
            tiles.decoded++;
            return [FLAT, FLAT, FLAT, FLAT];
        },
    };
    return tiles;
}

/** Frames of the walk view until the pages are idle; the work of each frame. */
async function settle(pages: Pages, wanted: Node[], frames = 300, eye = new Set<string>()): Promise<Work[]> {
    const works: Work[] = [];
    for (let i = 0; i < frames && (i === 0 || !pages.idle); i++) {
        works.push(pages.update(wanted, anchor, new Set(), eye));
        await new Promise(resolve => setTimeout(resolve));
    }
    return works;
}

describe('the page pools', () => {
    const { wanted } = select(cdlod({ anchor, above: 1.6 }, RANGE.least), BOUNDS, () => true);
    wanted.sort((a, b) => a.level - b.level);
    const eye = new Set(wanted.filter(node => distances({ anchor, above: 0 }, node)[0] <= EYE_M).map(key));

    it('synthesize the levels below the stored tiles from the deepest stored page, decode each tile once, and map every ready page', async () => {
        const tiles = source(), pages = new Pages(tiles, { stored: 400, synth: 600 }, BOUNDS, () => {});
        const works = await settle(pages, wanted);
        expect(pages.idle).toBe(true);
        expect(tiles.decoded).toBe(tiles.fetched.filter(name => Number(name.split('/')[0]) <= 12).length);
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

    it('fill the holes from one coarse tile, then load the tile under the eye, then the nearer tiles first', async () => {
        const tiles = source(), pages = new Pages(tiles, { stored: 400, synth: 600 }, BOUNDS, () => {});
        await settle(pages, wanted, 300, eye);
        const at = (z: number) => `${z}/${Math.floor((anchor.whole[0] + anchor.part[0]) / size(z))}/${Math.floor((anchor.whole[1] + anchor.part[1]) / size(z))}`;
        expect(tiles.fetched.slice(0, 2)).toEqual([at(6), at(13)]);
        const metres = tiles.fetched.map(name => name.split('/').map(Number)).filter(([z]) => z === 13).map(([level, x, y]) => distances({ anchor, above: 0 }, { level, x, y })[0]);
        expect(metres).toEqual([...metres].sort((a, b) => a - b));
    });

    it('let the eye field ask for its features and synthesize first', async () => {
        const tiles = source(), asked: string[] = [], known = new Set<string>();
        // Every node's features are known in the frame after it asks for them.
        const features: StampSource = {
            get: node => {
                if (known.has(key(node))) return { data: null, heights: false, mask: false };
                if (asked.includes(key(node))) known.add(key(node));
                else asked.push(key(node));
            },
            known: node => (known.has(key(node)) ? { data: null, heights: false, mask: false } : undefined),
        };
        const pages = new Pages(tiles, { stored: 400, synth: 600 }, BOUNDS, () => {}, features);
        const works = await settle(pages, wanted, 300, eye);
        // Of the synthesized pages (below the stored level 13), the eye field asks first.
        const synthesized = asked.filter(name => Number(name.split('/')[0]) > 13), field = synthesized.filter(name => eye.has(name));
        expect(field.length).toBeGreaterThan(0);
        expect(synthesized.slice(0, field.length)).toEqual(field);
        for (const work of works) {
            const made = work.syntheses.map(({ page }) => eye.has(key(page.node)));
            expect(made).toEqual([...made].sort((a, b) => Number(b) - Number(a)));
        }
    });

    it('let a ready stored page ask for its features before the synthesized pages behind it in the frame', async () => {
        // A worker with 8 requests in flight: the features of a node are known 3 frames after it asks.
        let frame = 0;
        const tiles = source(), asked = new Map<string, number>();
        const known = (node: Node) => (frame >= (asked.get(key(node)) ?? Infinity) + 3 ? { data: null, heights: false, mask: false } : undefined);
        const features: StampSource = {
            get: node => {
                if (!asked.has(key(node)) && [...asked.values()].filter(at => frame < at + 3).length < 8) asked.set(key(node), frame);
                return known(node);
            },
            known,
        };
        const pages = new Pages(tiles, { stored: 400, synth: 600 }, BOUNDS, () => {}, features);
        let stored = 0, synthesized = 0;
        for (; frame < 400 && !(frame && pages.idle); frame++) {
            const work = pages.update(wanted, anchor, new Set(), eye);
            if (work.fresh.some(page => page.kind === 'stored')) stored = frame;
            if (work.syntheses.length) synthesized = frame;
            await new Promise(resolve => setTimeout(resolve));
        }
        expect(pages.idle).toBe(true);
        expect(stored).toBeLessThan(synthesized);
    });

    it('load every missing input of a synthesis in the same frame', async () => {
        const at = (level: number) => ({ level, x: Math.floor((anchor.whole[0] + anchor.part[0]) / size(level)), y: Math.floor((anchor.whole[1] + anchor.part[1]) / size(level)) });
        const tiles = source(13), pages = new Pages(tiles, { stored: 400, synth: 600 }, BOUNDS, () => {});
        for (let i = 0; i < 50 && !pages.pages.get(key(at(14)))?.ready; i++) {
            pages.update([at(16)], anchor);
            await new Promise(resolve => setTimeout(resolve));
        }
        // The level-14 ancestor and its neighbours lie in the zoom-13 tiles about the eye's.
        const { x, y } = at(14), inputs = new Set([-1, 0, 1].flatMap(dy => [-1, 0, 1].map(dx => `13/${(x + dx) >> 1}/${(y + dy) >> 1}`)));
        inputs.delete(key(at(13)));
        const before = tiles.fetched.length;
        pages.update([at(16)], anchor);
        expect(new Set(tiles.fetched.slice(before))).toEqual(inputs);
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

    it('stay at their size, count a shortage in the frame that wants too many pages, and evict none that it wants', async () => {
        const small = new Pages(source(), { stored: 400, synth: 20 }, BOUNDS, () => {});
        await settle(small, wanted, 60);
        const resident = [...small.pages.values()];
        expect(resident.filter(page => page.kind === 'synth').length).toBeLessThanOrEqual(20);
        expect(new Set(resident.map(page => page.slot)).size).toBe(resident.length);
        expect(small.shortage).toBeGreaterThan(0);
        expect(small.wanted.synth).toBeGreaterThan(20);

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
        // The arrival and the page that fills the hole at `far` find no slot.
        expect(pages.shortage).toBe(2);
    });

    it('keep the inputs of a synthesis that arrive together in a pool that just holds them', async () => {
        const [w, n] = worldPixel([7.6, 47.25], UNITS), [e, s] = worldPixel([8.95, 46.46], UNITS);
        const at = (level: number) => ({ level, x: Math.floor((anchor.whole[0] + anchor.part[0]) / size(level)), y: Math.floor((anchor.whole[1] + anchor.part[1]) / size(level)) });
        // The synthesis reads the stored page of level 13 and its eight neighbours, which no frame wants.
        const pages = new Pages(source(), { stored: 9, synth: 1 }, [w, n, e, s], () => {});
        const works = await settle(pages, [at(14)], 100);
        expect(works.flatMap(work => work.syntheses).map(({ page }) => key(page.node))).toEqual([key(at(14))]);
        expect(pages.idle).toBe(true);
    });
});

describe('the stamps', () => {
    const { wanted } = select(cdlod({ anchor, above: 1.6 }, RANGE.least), BOUNDS, () => true);
    wanted.sort((a, b) => a.level - b.level);
    /** Every node has a feature near it; its stamps are known one frame after a page asks for them. */
    const stamps = (): StampSource => {
        const asked = new Set<string>(), known = (node: Node) => (asked.has(key(node)) ? { data: new Uint32Array(8), heights: true, mask: true } : undefined);
        return { get: node => known(node) ?? void asked.add(key(node)), known };
    };

    it('draw a stored page as a stamped copy, keep its raw layer for the syntheses, and stamp a synthesized page as it is made', async () => {
        const pages = new Pages(source(), { stored: 400, synth: 600 }, BOUNDS, () => {}, stamps());
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

    it('queue no more stamp jobs in a frame than its buffers hold, of every kind', async () => {
        // The synthesized pages draw before their features are known, the stored pages wait for theirs; then every node has some.
        let known = false;
        const get = (node: Node) => (known ? { data: new Uint32Array(8), heights: true, mask: true } : node.level > 13 ? { data: null, heights: false, mask: false } : undefined);
        const late: StampSource = { get, known: get };
        const pages = new Pages(source(), { stored: 400, synth: 600 }, BOUNDS, () => {}, late, 128);
        await settle(pages, wanted, 100);
        known = true;
        const works = await settle(pages, wanted);
        expect(works.some(work => work.stamps.some(({ mode }) => mode === MODE.MASK))).toBe(true);
        expect(works.some(work => work.stamps.some(({ page }) => page.kind === 'stored'))).toBe(true);
        expect(Math.max(...works.map(work => work.stamps.length))).toBeLessThanOrEqual(ARRIVALS);
    });
});

describe('a turn', () => {
    // A flight 1 km above the ground over Engelberg, in the whole box of the bakes.
    const [w, n] = worldPixel([7.6, 47.25], UNITS), [e, s] = worldPixel([8.95, 46.46], UNITS);
    const box: [number, number, number, number] = [w, n, e, s];
    const rule = cdlod({ anchor, above: 1000 }, RANGE.least);
    const ex = anchor.whole[0] + anchor.part[0];
    const side = (east: boolean) => select(rule, box, ({ level, x }) => (east ? (x + 1) * size(level) > ex : x * size(level) < ex));

    it('finds a ready page under every node of the new view before a tile of it arrives', async () => {
        const pages = new Pages(source(), { stored: 600, synth: 600 }, box, () => {});
        const east = side(true).wanted.sort((a, b) => a.level - b.level);
        await settle(pages, east);
        expect(pages.idle).toBe(true);
        const west = side(false).nodes;
        expect(west.filter(node => !pages.covers(node))).toEqual([]);
    });
});
