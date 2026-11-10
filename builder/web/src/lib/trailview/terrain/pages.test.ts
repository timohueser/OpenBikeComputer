import { describe, expect, it } from 'vitest';
import { worldPixel } from '../../planner/layers/mercator';
import { anchorAt, UNITS } from '../frame/space';
import { pageData, PAGE } from '../stream/heights';
import type { TerrainSource } from '../stream/terrain';
import { cdlod, distances, EYE_M, key, MAP, RANGE, select, size, type Node } from './lod';
import { ARRIVALS, MODE, Pages, touches, type StampSource, type Work } from './pages';
import { stepQuality } from './terrain';

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

    it('serve the roads and the pages from one request of a tile', async () => {
        const tiles = source(), pages = new Pages(tiles, { stored: 400, synth: 600 }, BOUNDS, () => {});
        await Promise.all([pages.body(5, 1, 2), pages.body(5, 1, 2), pages.body(20, 1, 2)]);
        expect(await pages.body(5, 1, 2)).toBeDefined();
        expect(tiles.fetched.filter(name => name === '5/1/2')).toHaveLength(1);
        expect(await pages.body(20, 1, 2)).toBeUndefined();
        expect(tiles.fetched.filter(name => name === '20/1/2')).toHaveLength(1);
    });

    it('ask again for a tile that failed, and never call it absent, whoever met the failure first', async () => {
        const tiles = source();
        let failing = true;
        const fetch = tiles.fetch;
        tiles.fetch = async (z, x, y) => {
            if (failing) throw new Error('A tile failed.');
            return fetch(z, x, y);
        };
        const pages = new Pages(tiles, { stored: 400, synth: 600 }, BOUNDS, () => {}), byFetch = pages as unknown as { fetch(z: number, x: number, y: number): void; stored(node: { level: number; x: number; y: number }): string };
        await expect(pages.body(5, 1, 2)).rejects.toThrow();
        await expect(pages.body(5, 1, 2)).rejects.toThrow();
        byFetch.fetch(5, 2, 2);
        await new Promise(resolve => setTimeout(resolve));
        await expect(pages.body(5, 2, 2)).rejects.toThrow();
        failing = false;
        expect(await pages.body(5, 1, 2)).toBeDefined();
        expect(byFetch.stored({ level: 6, x: 4, y: 4 })).toBe('no');
        expect(await pages.body(5, 2, 2)).toBeDefined();
        expect(byFetch.stored({ level: 6, x: 4, y: 4 })).not.toBe('no');
        expect(tiles.fetched).toEqual(['5/1/2', '5/2/2']);
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

    it('leave out the farthest pages of the finest level first, draw their parents in their place, and change nothing else', async () => {
        const drawnOf = (pages: Pages) => wanted.filter(node => pages.pages.get(key(node))?.drawn !== undefined).map(key);
        const levels = (counts: Record<number, number>) => Array.from({ length: 19 }, (_, level) => counts[level] ?? 0);
        const cell = ({ level, x, y }: Node) => ({ level: 16, x: x >> (level - 16), y: y >> (level - 16) });
        const all = new Pages(source(), { stored: 400, synth: 600 }, BOUNDS, () => {});
        await settle(all, wanted);
        const synth = all.wanted.synth, full = new Pages(source(), { stored: 400, synth }, BOUNDS, () => {});
        await settle(full, wanted);
        expect(full.left).toEqual(levels({}));
        expect(wanted.every(node => full.pages.get(key(node))?.ready)).toBe(true);

        // One slot short: only the farthest page of level 18 leaves, and its parent draws there.
        const short = new Pages(source(), { stored: 400, synth: synth - 1 }, BOUNDS, () => {});
        await settle(short, wanted);
        expect(short.idle).toBe(true);
        expect(short.left).toEqual(levels({ 18: 1 }));
        const gone = drawnOf(full).filter(name => !drawnOf(short).includes(name));
        expect(gone).toHaveLength(1);
        expect(drawnOf(short)).toEqual(drawnOf(full).filter(name => name !== gone[0]));
        const ex = anchor.whole[0] + anchor.part[0], ey = anchor.whole[1] + anchor.part[1], s = size(18), left = wanted.find(node => key(node) === gone[0])!;
        const centre = (node: Node) => Math.hypot((node.x + 0.5) * s - ex, (node.y + 0.5) * s - ey);
        expect(left.level).toBe(18);
        expect(wanted.filter(node => node.level === 18).every(node => centre(node) <= centre(left))).toBe(true);
        expect(short.drawing(left)?.level).toBe(17);
        expect(short.fallback(cell(left), 18)).toBe(17);
        expect(stepQuality({ quality: 0, calm: 0 }, short.left, short.missing === 0, false).quality).toBe(0);

        // Too few slots for level 16: no page of 17 or 18 draws, the farthest of 16 leave, the eye's cell falls back past 17, and the quality falls.
        const coarse = wanted.filter(node => node.level <= 16 && node.level > 13).length, tiny = new Pages(source(), { stored: 400, synth: coarse - 1 }, BOUNDS, () => {});
        await settle(tiny, wanted);
        expect(tiny.left[16]).toBeGreaterThan(0);
        expect(wanted.filter(node => node.level >= 17).some(node => tiny.pages.get(key(node))?.drawn !== undefined)).toBe(false);
        expect(wanted.filter(node => node.level <= 14).every(node => tiny.pages.get(key(node))?.drawn !== undefined)).toBe(true);
        const eye = { level: 18, x: Math.floor(ex / s), y: Math.floor(ey / s) };
        expect(tiny.fallback(cell(eye), 18)).toBeLessThan(17);
        expect(stepQuality({ quality: 0, calm: 0 }, tiny.left, tiny.missing === 0, false).quality).toBe(1);
    });

    it('keep the same pages in a short pool whatever the order in which the tiles arrive', async () => {
        const all = new Pages(source(), { stored: 400, synth: 600 }, BOUNDS, () => {});
        await settle(all, wanted);
        const late = (delay: (name: string) => number) => {
            const tiles = source(), fetch = tiles.fetch;
            tiles.fetch = async (z, x, y) => {
                await new Promise(resolve => setTimeout(resolve, delay(`${z}/${x}/${y}`)));
                return fetch(z, x, y);
            };
            return tiles;
        };
        const hash = (name: string) => [...name].reduce((h, c) => (31 * h + c.charCodeAt(0)) % 7, 0);
        const kept = async (delay: (name: string) => number) => {
            const pages = new Pages(late(delay), { stored: 400, synth: all.wanted.synth - 12 }, BOUNDS, () => {});
            await settle(pages, wanted, 600);
            expect(pages.idle).toBe(true);
            return { drawn: wanted.filter(node => pages.pages.get(key(node))?.drawn !== undefined).map(key).sort(), left: [...pages.left] };
        };
        const first = await kept(hash), second = await kept(name => 6 - hash(name));
        expect(first.left[18]).toBeGreaterThanOrEqual(12);
        expect(second).toEqual(first);
    });

    it('name in each frame the pages under every box whose read key changes: arrivals, syntheses, evictions and restamps', async () => {
        const ex = anchor.whole[0] + anchor.part[0], ey = anchor.whole[1] + anchor.part[1], s = size(16), order = (a: Node, b: Node) => a.level - b.level;
        const half = (east: boolean) => select(cdlod({ anchor, above: 1.6 }, RANGE.least), BOUNDS, ({ level, x }) => (east ? (x + 1) * size(level) > ex : x * size(level) < ex)).wanted.sort(order);
        const boxes = [-1, 0, 1].flatMap(j => [-1, 0, 1].map((i): [number, number, number, number] => {
            const x = (Math.floor(ex / s) + i) * s, y = (Math.floor(ey / s) + j) * s;
            return [x + 100, y + 100, x + s - 100, y + s - 100];
        }));
        // A pool that cannot hold both halves: a turn evicts.
        const pages = new Pages(source(), { stored: 400, synth: 90 }, BOUNDS, () => {});
        const read = (box: [number, number, number, number]) => `${pages.reads(box, 18, true)};${pages.reads(box, 18, false)}`;
        let keys = boxes.map(read), changes = 0;
        for (let i = 0; i < 240; i++) {
            if (i === 150) pages.restamp({ level: 16, x: Math.floor(ex / s), y: Math.floor(ey / s) });
            const work = pages.update(half(Math.floor(i / 60) % 2 === 0), anchor);
            const now = boxes.map(read);
            boxes.forEach((box, k) => {
                if (now[k] === keys[k]) return;
                changes++;
                expect(work.touched.some(node => touches(node, box))).toBe(true);
            });
            keys = now;
            await new Promise(resolve => setTimeout(resolve));
        }
        expect(changes).toBeGreaterThan(boxes.length);
        // A frame that changes nothing names nothing.
        await settle(pages, half(false));
        expect(pages.update(half(false), anchor).touched).toEqual([]);
    });

    it('keep the stored pages that a synthesis reads when a page arrives in the same frame', async () => {
        const [w, n] = worldPixel([7.6, 47.25], UNITS), [e, s] = worldPixel([8.95, 46.46], UNITS);
        const at = (level: number) => ({ level, x: Math.floor((anchor.whole[0] + anchor.part[0]) / size(level)), y: Math.floor((anchor.whole[1] + anchor.part[1]) / size(level)) });
        const stored = at(13), around = [-1, 0, 1].flatMap(dy => [-1, 0, 1].map(dx => ({ level: 13, x: stored.x + dx, y: stored.y + dy })));
        // The pool holds the stored ancestor of the synthesis and its eight neighbours, and the page that fills the hole
        // until they come; zoom 13 is known absent.
        const pages = new Pages(source(), { stored: 10, synth: 10 }, [w, n, e, s], () => {});
        await settle(pages, [...around, at(14)]);
        expect(around.every(node => pages.pages.get(key(node))?.ready)).toBe(true);
        // A stored page far away is decoded and waits to arrive.
        const far = { level: 13, x: stored.x + 6, y: stored.y };
        pages.update([far], anchor);
        await new Promise(resolve => setTimeout(resolve));
        // The arrival takes the slot of the page that filled the hole, not one that the synthesis reads.
        const work = pages.update([at(15), far], anchor);
        expect(work.syntheses.map(({ page, stored }) => [key(page.node), stored])).toEqual([[key(at(15)), 13]]);
        expect([...around, far].every(node => pages.pages.get(key(node))?.ready)).toBe(true);
        expect(pages.left.every(count => count === 0)).toBe(true);
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
        const keys = new Set(wanted.map(key));
        pages.update(wanted, anchor);
        expect(pages.wanted.stored).toBe(2 * stored.filter(page => keys.has(key(page.node))).length);
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
