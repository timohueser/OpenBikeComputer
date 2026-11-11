import { readFileSync } from 'node:fs';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { CELL_LEVEL, MODEL_ROW, Records } from './model';
import { DeviceCache } from './device-cache';
import { FakeCaches } from '../../../../test-support/fake-caches';
import { HEAP_LIMIT } from '../plan/plan';
import type { PlanRequest } from '../plan/plan-worker';

/** A plan worker that answers `start` and keeps the cell builds for the test to answer. */
class FakeWorker {
    static all: FakeWorker[] = [];
    onmessage?: (event: { data: unknown }) => void;
    onerror?: unknown;
    builds: Extract<PlanRequest, { type: 'cell' }>[] = [];
    meshes: Extract<PlanRequest, { type: 'mesh' }>[] = [];
    dead = false;

    constructor() {
        FakeWorker.all.push(this);
    }

    postMessage(message: PlanRequest): void {
        if (message.type === 'start') queueMicrotask(() => this.onmessage?.({ data: { type: 'ready' } }));
        else if (message.type === 'cell') this.builds.push(message);
        else if (message.type === 'mesh') this.meshes.push(message);
    }

    /** Answers a build with a block of `rows` rows, a rim, the worker's heap and a far block or the crate's refusal of it. */
    answer(build: { id: number }, rows: number, rim = new Uint32Array(0), heap = 2 ** 20, far: Uint32Array | { error: string } = new Uint32Array(0)): void {
        this.onmessage?.({ data: { id: build.id, ok: true, words: new Uint32Array(rows * MODEL_ROW), rim, far, ms: 1, heap } });
    }

    terminate(): void {
        this.dead = true;
    }
}

const settle = () => new Promise(resolve => setTimeout(resolve, 0));
// Two cells of the fixture's zoom-13 tile 4286/2888.
const A = { level: CELL_LEVEL, x: 4286 * 8, y: 2888 * 8 }, B = { level: CELL_LEVEL, x: 4286 * 8 + 1, y: 2888 * 8 };

describe('the records store', () => {
    afterEach(() => vi.unstubAllGlobals());

    /** A records store with its two workers ready; `features` gives a feature tile's body. */
    async function store(features: () => Promise<ArrayBuffer> = async () => new ArrayBuffer(4)) {
        FakeWorker.all = [];
        vi.stubGlobal('Worker', FakeWorker);
        vi.stubGlobal('fetch', async () => new Response(readFileSync(new URL('../plan/pkg/obc_ground_plan_bridge_bg.wasm', import.meta.url))));
        const records = new Records(features, async () => undefined, () => {}, new AbortController().signal);
        for (let i = 0; i < 20 && FakeWorker.all.length < 2; i++) await settle();
        await settle();
        /** The builds that the workers hold, taken from them. */
        const posted = () => FakeWorker.all.flatMap(worker => worker.builds.splice(0).map(build => ({ worker, build })));
        return { records, posted };
    }
    const at = (dx: number, dy = 0) => ({ level: CELL_LEVEL, x: B.x + dx, y: B.y + dy });

    it('gives the rows to the nearest cells, asks again for a block without rows once it is nearer, counts a refused far block, and nothing waits once the records failed', async () => {
        const { records, posted } = await store();
        const frame = async (cells: { node: typeof A; distance: number }[]) => {
            records.update(cells);
            await settle();
            return posted();
        };
        // Both have the reserve; their blocks of 1200 rows, over the reserve, do not both fit the 2047 rows.
        const [a, b] = await frame([{ node: A, distance: 100 }, { node: B, distance: 200 }]);
        expect(records.waits(B)).toBe(true);
        a.worker.answer(a.build, 1200);
        b.worker.answer(b.build, 1200, undefined, undefined, { error: 'a run of too many samples' });
        expect([records.stats.cells, records.stats.failed, records.stats.refused, records.stats.large]).toEqual([1, 1, 1, 2]);
        // B's known size keeps it out of the target: its pages draw without records, and it is not built again.
        expect(await frame([{ node: A, distance: 100 }, { node: B, distance: 200 }])).toEqual([]);
        expect(records.waits(B)).toBe(false);
        // The eye moves on: B comes first, and builds again; A leaves for it.
        const [again] = await frame([{ node: B, distance: 50 }, { node: A, distance: 150 }]);
        expect(again.build.cell).toMatchObject({ x: B.x, y: B.y });
        expect(records.waits(B)).toBe(true);
        records.takeChanged();
        again.worker.answer(again.build, 1200);
        expect(records.takeChanged()).toEqual([A, B]);
        expect([records.stats.cells, records.stats.failed, records.loading]).toEqual([1, 0, false]);
        // Without feature tiles, no page waits and the store does not count as loading.
        const none = new Records(undefined, async () => undefined, () => {}, new AbortController().signal);
        none.update([{ node: A, distance: 100 }]);
        expect([none.waits(A), none.loading, none.stats.error]).toEqual([false, false, 'the release has no feature tiles']);
    });

    it('starts a build only with the reserve free after the nearer blocks, and holds a block that arrives before them', async () => {
        const { records, posted } = await store();
        // Seven cells in a row, 100 m apart: the reserve of 384 rows fits five times into the 2047 rows.
        const cells = [1, 2, 3, 4, 5, 6, 7].map(i => ({ node: at(4 * i), distance: 100 * i }));
        const open = new Map<number, ReturnType<typeof posted>[number]>();
        /** A frame; returns the cells whose builds went to a worker in it, as their place in the row. */
        const frame = async () => {
            records.update(cells);
            await settle();
            const sent = posted().map(job => [(job.build.cell.x - B.x) / 4, job] as const);
            sent.forEach(([i, job]) => open.set(i, job));
            return sent.map(([i]) => i).sort();
        };
        const answer = (i: number, rows: number) => open.get(i)!.worker.answer(open.get(i)!.build, rows);
        expect(await frame()).toEqual([1, 2]);
        // With 700 rows of the first, the reserves of the next three leave none for the fifth: it is not asked for.
        answer(1, 700);
        expect(await frame()).toEqual([3]);
        answer(3, 300);
        expect(await frame()).toEqual([4]);
        // The fourth block arrives before the second. It fits only if the second is small: it waits, without rows, and its pages wait.
        answer(4, 900);
        expect([records.stats.cells, records.waits(at(16)), records.waits(at(20)), records.loading]).toEqual([2, true, false, true]);
        answer(2, 100);
        // 2000 rows are known: the fourth has its rows without a second build, and 47 rows are no reserve for the fifth.
        expect(await frame()).toEqual([]);
        expect([records.stats.cells, records.stats.builds, records.stats.refused, records.loading]).toEqual([4, 4, 0, false]);
        expect(cells.map(({ node }) => records.block(node) !== undefined)).toEqual([true, true, true, true, false, false, false]);
    });

    it('fills the rows past the reserve one cell at a time while the mean block fits, and counts the block that ends the rank', async () => {
        const cells = [1, 2, 3, 4, 5, 6, 7, 8, 9].map(i => ({ node: at(4 * i), distance: 100 * i }));
        /** A load in which each block has 280 rows, and the seventh has `seventh`: the cells built, and the store. */
        const load = async (seventh: number) => {
            const { records, posted } = await store();
            const built: number[] = [];
            for (let frame = 0; frame < 12; frame++) {
                records.update(cells);
                await settle();
                const sent = posted();
                // Past the reserve, a build starts only when no other is on its way.
                if (sent.some(({ build }) => (build.cell.x - B.x) / 4 > 6)) expect([sent.length, records.stats.building]).toEqual([1, 1]);
                for (const { worker, build } of sent) {
                    const i = (build.cell.x - B.x) / 4;
                    built.push(i);
                    worker.answer(build, i === 7 ? seventh : 280);
                }
            }
            return { built, records };
        };
        // Six blocks of 280 rows leave 367: no reserve, but the mean fits, so the seventh is tried. Its 250 rows leave 117:
        // the mean of 276 does not fit, so the eighth is not tried.
        const fits = await load(250);
        expect(fits.built).toEqual([1, 2, 3, 4, 5, 6, 7]);
        expect([fits.records.stats.cells, fits.records.stats.refused, fits.records.loading]).toEqual([7, 0, false]);
        // A seventh block of 380 rows finds none: it is counted, and it ends the rank.
        const ends = await load(380);
        expect(ends.built).toEqual([1, 2, 3, 4, 5, 6, 7]);
        expect([ends.records.stats.cells, ends.records.stats.refused, ends.records.stats.large, ends.records.waits(at(32)), ends.records.loading]).toEqual([6, 1, 0, false, false]);
    });

    it('builds the eye\'s cell first, then its neighbours, the nearer first, then the other cells, also when the inputs of the eye\'s cell come last', async () => {
        // The first tile read, of the eye's cell, takes longer than the others.
        let first = true;
        const { records, posted } = await store(async () => {
            const slow = first;
            first = false;
            if (slow) await new Promise(resolve => setTimeout(resolve, 20));
            return new ArrayBuffer(4);
        });
        const eye = at(3, 3), mesh = [{ node: eye, distance: 0 }];
        // The eye stands in the north-east of its cell.
        const near = ({ x, y }: typeof A) => Math.hypot(Math.max(0, eye.x - x) * 300 + Math.max(0, x - eye.x) * 100, Math.max(0, y - eye.y) * 200 + Math.max(0, eye.y - y) * 50);
        const order: number[][] = [], running: ReturnType<typeof posted> = [];
        for (let round = 0; round < 12; round++) {
            records.update([{ node: eye, distance: 0 }, { node: at(6, 3), distance: 250 }], mesh, false, near);
            await new Promise(resolve => setTimeout(resolve, 30));
            const sent = posted();
            order.push(...sent.map(({ build: { cell } }) => [cell.x - eye.x, cell.y - eye.y]));
            running.push(...sent);
            // The build that started first ends first.
            const done = running.shift();
            done?.worker.answer(done.build, 1);
        }
        // A worker stays free for the eye's cell while the nearest neighbour, whose inputs came first, starts.
        expect(order).toEqual([[0, -1], [0, 0], [1, 0], [1, -1], [0, 1], [1, 1], [-1, 0], [-1, -1], [-1, 1], [3, 0]]);
        expect(FakeWorker.all.flatMap(w => w.meshes)).toHaveLength(1);
    });

    it('does not build a cell that left before its build started, gives no rows to one that left while it ran, and builds the cells of a jump in their order', async () => {
        const { records, posted } = await store();
        const far = at(40, 40);
        records.update([{ node: B, distance: 0 }], [{ node: B, distance: 0 }]);
        await settle();
        const old = posted();
        expect([old.length, records.stats.building]).toEqual([2, 4]);
        // The eye jumps: the two builds that wait for a worker leave, and two cells about the new eye wait in their place.
        records.update([{ node: far, distance: 0 }], [{ node: far, distance: 0 }]);
        await settle();
        expect([posted(), records.stats.building, records.waits(B), records.waits(far)]).toEqual([[], 4, false, true]);
        // The old builds arrive: no block takes rows, none counts as refused, and the eye's new cell is the next build.
        const next: number[][] = [];
        for (const { worker, build } of old) {
            worker.answer(build, 1);
            await settle();
            next.push(...posted().map(({ build: { cell } }) => [cell.x - far.x, cell.y - far.y]));
        }
        expect([records.stats.cells, records.stats.builds, records.stats.refused, records.block(B)]).toEqual([0, 2, 0, undefined]);
        expect(next).toHaveLength(2);
        expect(next[0]).toEqual([0, 0]);
        expect(Math.max(...next[1].map(Math.abs))).toBe(1);
    });

    it('asks for a mesh once each of the eight neighbours of the cell has a rim, with all their rims, and once only, as the workers answer', async () => {
        FakeWorker.all = [];
        vi.stubGlobal('Worker', FakeWorker);
        vi.stubGlobal('fetch', async () => new Response(readFileSync(new URL('../plan/pkg/obc_ground_plan_bridge_bg.wasm', import.meta.url))));
        const records = new Records(async () => new ArrayBuffer(4), async () => undefined, () => {}, new AbortController().signal);
        for (let i = 0; i < 20 && FakeWorker.all.length < 2; i++) await settle();
        await settle();
        const mesh = [{ node: B, distance: 0 }], rims = new Map<string, number>();
        let rim = 1;
        // One frame: each answer asks for the next cells, and the last rim for the mesh.
        records.update([{ node: B, distance: 0 }], mesh);
        for (let round = 0; round < 12; round++) {
            await settle();
            // Until the eight neighbours have rims, no mesh is asked for.
            if (rims.size < 9) expect(FakeWorker.all.flatMap(w => w.meshes)).toEqual([]);
            for (const worker of FakeWorker.all) for (const build of worker.builds.splice(0)) {
                rims.set(`${build.cell.x}/${build.cell.y}`, rim);
                worker.answer(build, 1, new Uint32Array([rim++]));
            }
        }
        const asked = FakeWorker.all.flatMap(w => w.meshes);
        expect(rims.size).toBe(9);
        expect(asked.map(m => [m.cell.x, m.cell.y])).toEqual([[B.x, B.y]]);
        expect([...asked[0].rims].sort((a, b) => a - b)).toEqual([...rims.values()].sort((a, b) => a - b));
    });

    it('builds a mesh cell and its neighbours, as near as it, before a farther target cell', async () => {
        FakeWorker.all = [];
        vi.stubGlobal('Worker', FakeWorker);
        vi.stubGlobal('fetch', async () => new Response(readFileSync(new URL('../plan/pkg/obc_ground_plan_bridge_bg.wasm', import.meta.url))));
        const records = new Records(async () => new ArrayBuffer(4), async () => undefined, () => {}, new AbortController().signal);
        for (let i = 0; i < 20 && FakeWorker.all.length < 2; i++) await settle();
        await settle();
        const far = { level: CELL_LEVEL, x: B.x + 4, y: B.y };
        const sent: number[][] = [];
        for (let round = 0; round < 2; round++) {
            records.update([{ node: far, distance: 300 }], [{ node: B, distance: 0 }]);
            await settle();
            for (const worker of FakeWorker.all) for (const build of worker.builds.splice(0)) {
                sent.push([build.cell.x - B.x, build.cell.y - B.y]);
                worker.answer(build, 1);
            }
        }
        // Two workers, and two builds asked for per worker: all four go to the mesh's ring.
        expect(sent).toHaveLength(4);
        expect(sent.every(([dx, dy]) => Math.abs(dx) <= 1 && Math.abs(dy) <= 1)).toBe(true);
    });

    it('keeps a full worker while it holds the input of a mesh not asked yet, and draws the pages of a cell whose kept input was lost', async () => {
        FakeWorker.all = [];
        vi.stubGlobal('Worker', FakeWorker);
        vi.stubGlobal('fetch', async () => new Response(readFileSync(new URL('../plan/pkg/obc_ground_plan_bridge_bg.wasm', import.meta.url))));
        const records = new Records(async () => new ArrayBuffer(4), async () => undefined, () => {}, new AbortController().signal);
        for (let i = 0; i < 20 && FakeWorker.all.length < 2; i++) await settle();
        await settle();
        const mesh = [{ node: B, distance: 0 }];
        let rim = 1, builtB = 0;
        // Each build leaves the heap over HEAP_LIMIT, as a town cell can; each live worker answers one build a frame.
        const frames = async (count: number) => {
            for (let frame = 0; frame < count && !FakeWorker.all.some(w => !w.dead && w.meshes.length); frame++) {
                records.update([{ node: B, distance: 0 }], mesh);
                await settle();
                await settle();
                for (const worker of FakeWorker.all.filter(w => !w.dead)) {
                    const build = worker.builds.shift();
                    if (build && build.cell.x === B.x && build.cell.y === B.y) builtB++;
                    if (build) worker.answer(build, 1, new Uint32Array([rim++]), HEAP_LIMIT + 1);
                }
            }
            return FakeWorker.all.find(w => !w.dead && w.meshes.length);
        };
        const holder = (await frames(30))!;
        expect([builtB, holder.meshes.length, records.meshComing(B)]).toEqual([1, 1, true]);
        // The worker lost the input: the pages draw, and the cell builds again for its mesh.
        holder.onmessage?.({ data: { id: holder.meshes.shift()!.id, ok: true, gone: true } });
        expect([records.meshComing(B), records.loading]).toEqual([false, true]);
        const again = (await frames(30))!;
        expect(builtB).toBe(2);
        const restarts = records.stats.restarts;
        again.onmessage?.({ data: { id: again.meshes[0].id, ok: true, mesh: new ArrayBuffer(4), ms: 1 } });
        expect(records.settled(B)).toBe(true);
        // Idle, full, and with no input to keep: the worker ends with its answer.
        expect(records.stats.restarts).toBeGreaterThan(restarts);
    });

    it('names a tile to a worker only once a build of it answered, sends every body after a miss, and every body to a new worker', async () => {
        FakeWorker.all = [];
        vi.stubGlobal('Worker', FakeWorker);
        vi.stubGlobal('fetch', async () => new Response(readFileSync(new URL('../plan/pkg/obc_ground_plan_bridge_bg.wasm', import.meta.url))));
        const records = new Records(async () => new ArrayBuffer(4), async () => undefined, () => {}, new AbortController().signal);
        for (let i = 0; i < 20 && FakeWorker.all.length < 2; i++) await settle();
        await settle();
        // Cells in the middle of tile 4286/2888 read it alone; cells at its east edge read 4287/2888 too.
        const [C, D, E] = [3, 4, 5].map(dx => ({ level: CELL_LEVEL, x: A.x + dx, y: A.y + 3 }));
        const [F, G] = [3, 4].map(dy => ({ level: CELL_LEVEL, x: A.x + 7, y: A.y + dy }));
        const bodies = (build: { tiles: { body?: ArrayBuffer }[] }) => build.tiles.map(({ body }) => !!body);
        const ask = async (...nodes: (typeof A)[]) => {
            records.update(nodes.map(node => ({ node, distance: 0 })));
            await settle();
            return FakeWorker.all.filter(w => !w.dead).flatMap(worker => worker.builds.splice(0).map(build => ({ worker, build })));
        };
        // No worker parsed the tile: each of the two builds carries the body to its worker.
        const [c, d] = await ask(C, D);
        expect([bodies(c.build), bodies(d.build), c.worker === d.worker]).toEqual([[true], [true], false]);
        c.worker.answer(c.build, 1);
        d.worker.answer(d.build, 1);
        const [f] = await ask(C, D, F);
        expect(bodies(f.build)).toEqual([false, true]);
        f.worker.answer(f.build, 1);
        // Only F's worker parsed 4287/2888: it gets G with both tiles by name.
        const [g] = await ask(C, D, F, G);
        expect([g.worker === f.worker, bodies(g.build)]).toEqual([true, [false, false]]);
        // The worker let tile 4287/2888 go: the job comes again with every body, also the held one, so it cannot miss again.
        const worker = g.worker;
        worker.onmessage?.({ data: { id: g.build.id, ok: true, missing: new Uint32Array([4287, 2888]) } });
        await settle();
        const [again] = worker.builds.splice(0);
        expect([again.id, bodies(again)]).toEqual([g.build.id, [true, true]]);
        // A trap ends the worker and fails its cell: the worker after it gets the body with its first job.
        worker.onmessage?.({ data: { id: again.id, ok: false, error: 'unreachable', trapped: true } });
        for (let i = 0; i < 5; i++) await settle();
        records.update([C, D, F, G, E].map(node => ({ node, distance: 0 })));
        await settle();
        const [fresh] = FakeWorker.all[2].builds;
        expect([worker.dead, bodies(fresh)]).toEqual([true, [true]]);
    });

    it('frees the job of a miss whose cell left', async () => {
        FakeWorker.all = [];
        vi.stubGlobal('Worker', FakeWorker);
        vi.stubGlobal('fetch', async () => new Response(readFileSync(new URL('../plan/pkg/obc_ground_plan_bridge_bg.wasm', import.meta.url))));
        const records = new Records(async () => new ArrayBuffer(4), async () => undefined, () => {}, new AbortController().signal);
        for (let i = 0; i < 20 && FakeWorker.all.length < 2; i++) await settle();
        await settle();
        const C = { level: CELL_LEVEL, x: A.x + 3, y: A.y + 3 }, worker = FakeWorker.all[0];
        records.update([{ node: C, distance: 0 }]);
        await settle();
        worker.answer(worker.builds.shift()!, 1);
        // The built cell builds again for its mesh, and leaves with a lost device before the worker misses its tile.
        records.update([{ node: C, distance: 0 }], [{ node: C, distance: 0 }]);
        await settle();
        const rebuild = worker.builds.find(({ cell }) => cell.x === C.x && cell.y === C.y)!;
        records.lost();
        records.update([]);
        worker.onmessage?.({ data: { id: rebuild.id, ok: true, missing: new Uint32Array([4286, 2888]) } });
        for (const w of FakeWorker.all) for (const build of w.builds.splice(0)) if (build !== rebuild) w.answer(build, 1);
        for (let i = 0; i < 5; i++) await settle();
        expect(records.stats.building).toBe(0);
    });
});

describe('the device cache of built cells', () => {
    afterEach(() => vi.unstubAllGlobals());
    const WASM = new Uint8Array(readFileSync(new URL('../plan/pkg/obc_ground_plan_bridge_bg.wasm', import.meta.url)));

    /** An engine's records store on `wasm` and cache key `release` + `version`, its block uploads and meshes, with its two workers ready. */
    async function engine(wasm: Uint8Array<ArrayBuffer>, release: string, version = 'plan a') {
        FakeWorker.all = [];
        vi.stubGlobal('Worker', FakeWorker);
        vi.stubGlobal('fetch', async () => new Response(wasm));
        // A page loads the bridge once: a new module graph is a new page.
        vi.resetModules();
        const { Records } = await import('./model');
        const meshes: ArrayBuffer[] = [];
        const records = new Records(async () => new ArrayBuffer(4), async () => undefined, () => {}, new AbortController().signal, mesh => meshes.push(mesh.bytes), () => {}, { release, version });
        for (let i = 0; i < 20 && FakeWorker.all.length < 2; i++) await settle();
        await settle();
        const blocks = () => {
            const out: number[][] = [];
            const device = { queue: { writeTexture: (at: { origin?: number[] }, words: Uint32Array) => at.origin && out.push([...words]) } };
            records.write(device as unknown as GPUDevice, {} as GPUTexture, [0, 0]);
            return out;
        };
        return { records, meshes, blocks };
    }

    /**
     * Frames about B, with its mesh unless `mesh` is false; the workers answer each build and mesh with bytes of
     * the cell, a build of `rows` rows of B, and a failure for the cell `fails`. Returns the cells built, as `x-y` from B.
     */
    async function load({ records }: Awaited<ReturnType<typeof engine>>, { mesh = true, rows = 1, fails = '' } = {}) {
        const built: string[] = [];
        const pause = () => new Promise(resolve => setTimeout(resolve, 10));
        for (let frame = 0; frame < 30 && (frame < 8 || records.stats.building); frame++) {
            records.update([{ node: B, distance: 0 }], mesh ? [{ node: B, distance: 0 }] : []);
            await pause();
            for (const worker of FakeWorker.all) {
                for (const { id, cell } of worker.builds.splice(0)) {
                    const name = `${cell.x - B.x}/${cell.y - B.y}`;
                    built.push(name);
                    if (name === fails) {
                        worker.onmessage?.({ data: { id, ok: false, error: 'a fetch failed', trapped: false } });
                        continue;
                    }
                    const words = Uint32Array.from({ length: (name === '0/0' ? rows : 1) * MODEL_ROW + 7 }, (_, i) => cell.x * 31 + cell.y * 17 + i);
                    // No labels, so no mouths.
                    words[2] = 0;
                    worker.onmessage?.({ data: { id, ok: true, words, rim: new Uint32Array([cell.x, cell.y]), far: new Uint32Array(0), ms: 1, heap: 2 ** 20 } });
                }
                for (const { id, rims } of worker.meshes.splice(0)) worker.onmessage?.({ data: { id, ok: true, mesh: new Uint32Array([...rims, 0x4f424d03]).buffer, ms: 1 } });
            }
        }
        // The cache writes after each build: wait until its bytes stop changing.
        for (let i = 0, last = -1; i < 100 && records.stats.cacheBytes !== last; i++) {
            last = records.stats.cacheBytes;
            await pause();
        }
        expect(records.settled(B)).toBe(true);
        return built;
    }

    it('gives a cell and its mesh as they were built, and misses with another wasm, input preparation or release', async () => {
        vi.stubGlobal('caches', new FakeCaches());
        const first = await engine(WASM, 'release a');
        expect(await load(first)).toHaveLength(9);
        const again = await engine(WASM, 'release a');
        expect(await load(again)).toEqual([]);
        // Blocks take rows in the order that they arrive.
        const blocks = (side: typeof first) => side.blocks().map(words => words.join()).sort();
        expect(again.records.stats.hits).toBe(9);
        // toStrictEqual compares the bytes of ArrayBuffers; toEqual does not.
        expect(again.meshes).toStrictEqual(first.meshes);
        expect(blocks(again)).toEqual(blocks(first));
        // A custom section changes the wasm's bytes, as a change of the crate or its rule tables does.
        expect(await load(await engine(new Uint8Array([...WASM, 0, 2, 1, 0x78]), 'release a'))).toHaveLength(9);
        expect(await load(await engine(WASM, 'release a', 'plan b'))).toHaveLength(9);
        expect(await load(await engine(WASM, 'release b'))).toHaveLength(9);
    });

    it('places a cached block without room and builds it no more, and keeps no mesh that lacks a failed neighbour\'s rim, which it counts', async () => {
        vi.stubGlobal('caches', new FakeCaches());
        // A block of B larger than the texture: it never finds room, so it never has a mesh.
        await load(await engine(WASM, 'release a'), { mesh: false, rows: 2100 });
        expect(await load(await engine(WASM, 'release a'))).not.toContain('0/0');
        vi.stubGlobal('caches', new FakeCaches());
        const partial = await engine(WASM, 'release a');
        expect(await load(partial, { fails: '1/1' })).toContain('1/1');
        expect([partial.meshes.length, partial.records.stats.failed]).toEqual([1, 1]);
        // B's block comes from the cache, its mesh does not: B builds again for its mesh's input.
        expect(await load(await engine(WASM, 'release a'))).toEqual(['0/0', '1/1']);
    });

    it('drops a corrupt or partial entry, and the least recently used past the cap', async () => {
        vi.stubGlobal('caches', new FakeCaches());
        const cache = (await DeviceCache.open('cells', 'release', 'build', 3000))!;
        const words = Uint32Array.from({ length: 300 }, (_, i) => i);
        for (const x of [1, 2]) await cache.write(`${x}/0/cell`, [words, new Uint32Array([x])]);
        expect((await cache.read('1/0/cell'))?.map(part => [...part])).toEqual([[...words], [1]]);
        await cache.write('3/0/cell', [words, new Uint32Array([3])]);
        // Three entries of a header, a digest and 301 words do not fit 3000 bytes: the second, used least recently, went.
        expect([await cache.read('2/0/cell'), cache.bytes]).toEqual([undefined, 2 * 4 * (4 + 8 + 301)]);
        const [name] = await caches.keys(), entries = (globalThis.caches as unknown as FakeCaches).caches.get(name)!;
        const corrupt = entries.get('https://cache.invalid/1/0/cell')!, partial = entries.get('https://cache.invalid/3/0/cell')!;
        new Uint32Array(corrupt.body)[20] ^= 1;
        partial.body = partial.body.slice(0, 400);
        expect([await cache.read('1/0/cell'), await cache.read('3/0/cell'), entries.size, cache.bytes]).toEqual([undefined, undefined, 0, 0]);
    });
});
