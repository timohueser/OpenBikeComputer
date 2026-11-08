import { readFileSync } from 'node:fs';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { CELL_LEVEL, MODEL_ROW, Records } from './model';
import { CellCache } from '../plan/cache';
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

    /** Answers a build with a block of `rows` rows, a rim and the worker's heap. */
    answer(build: { id: number }, rows: number, rim = new Uint32Array(0), heap = 2 ** 20): void {
        this.onmessage?.({ data: { id: build.id, ok: true, words: new Uint32Array(rows * MODEL_ROW), rim, ms: 1, heap } });
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

    it('counts known blocks in the target, asks again for a block without room once a farther cell can leave, and nothing waits once the records failed', async () => {
        FakeWorker.all = [];
        vi.stubGlobal('Worker', FakeWorker);
        vi.stubGlobal('fetch', async () => new Response(readFileSync(new URL('../plan/pkg/obc_ground_plan_bridge_bg.wasm', import.meta.url))));
        const records = new Records(async () => new ArrayBuffer(4), async () => undefined, () => {}, new AbortController().signal);
        for (let i = 0; i < 20 && !FakeWorker.all.length; i++) await settle();
        await settle();
        const worker = FakeWorker.all[0];
        const frame = async (cells: { node: typeof A; distance: number }[]) => {
            records.update(cells);
            await settle();
            return worker.builds.splice(0);
        };
        // Both fit the guess; their blocks of 1200 rows do not both fit the 2047 rows.
        const [a, b] = await frame([{ node: A, distance: 100 }, { node: B, distance: 200 }]);
        expect(records.waits(B)).toBe(true);
        worker.answer(a, 1200);
        worker.answer(b, 1200);
        expect(records.stats.cells).toBe(1);
        // B's known size keeps it out of the target: its pages draw without records, and it is not built again.
        expect(await frame([{ node: A, distance: 100 }, { node: B, distance: 200 }])).toEqual([]);
        expect(records.waits(B)).toBe(false);
        // The eye moves on: B comes first, A leaves for it, and B builds again.
        const [again] = await frame([{ node: B, distance: 50 }, { node: A, distance: 150 }]);
        expect(again.cell).toMatchObject({ x: B.x, y: B.y });
        expect(records.waits(B)).toBe(true);
        records.takeChanged();
        worker.answer(again, 1200);
        expect(records.takeChanged()).toEqual([A, B]);
        expect(records.stats.cells).toBe(1);
        expect(records.loading).toBe(false);
        // Without feature tiles, no page waits and the store does not count as loading.
        const none = new Records(undefined, async () => undefined, () => {}, new AbortController().signal);
        none.update([{ node: A, distance: 100 }]);
        expect([none.waits(A), none.loading, none.stats.error]).toEqual([false, false, 'the release has no feature tiles']);
    });

    it('asks for a mesh once each of the eight neighbours of the cell has a rim, with all their rims, and once only', async () => {
        FakeWorker.all = [];
        vi.stubGlobal('Worker', FakeWorker);
        vi.stubGlobal('fetch', async () => new Response(readFileSync(new URL('../plan/pkg/obc_ground_plan_bridge_bg.wasm', import.meta.url))));
        const records = new Records(async () => new ArrayBuffer(4), async () => undefined, () => {}, new AbortController().signal);
        for (let i = 0; i < 20 && FakeWorker.all.length < 2; i++) await settle();
        await settle();
        const mesh = [{ node: B, distance: 0 }], rims = new Map<string, number>();
        let rim = 1;
        for (let frame = 0; frame < 12; frame++) {
            records.update([{ node: B, distance: 0 }], mesh);
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
        records.update([{ node: far, distance: 300 }], [{ node: B, distance: 0 }]);
        await settle();
        const asked = FakeWorker.all.flatMap(w => w.builds).map(({ cell }) => [cell.x - B.x, cell.y - B.y]);
        // Two workers hold two builds each: all go to the mesh's ring.
        expect(asked).toHaveLength(4);
        expect(asked.every(([dx, dy]) => Math.abs(dx) <= 1 && Math.abs(dy) <= 1)).toBe(true);
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
        again.onmessage?.({ data: { id: again.meshes[0].id, ok: true, mesh: new ArrayBuffer(4), ms: 1 } });
        expect(records.settled(B)).toBe(true);
        // Idle, full, and with no input to keep: the next frame ends the worker.
        const restarts = records.stats.restarts;
        records.update([{ node: B, distance: 0 }], mesh);
        expect(records.stats.restarts).toBeGreaterThan(restarts);
    });
});

/** Cache Storage in memory: what a put keeps survives an engine, as on a device. */
class FakeCaches {
    caches = new Map<string, Map<string, { body: ArrayBuffer; headers: Headers }>>();

    async open(name: string) {
        const entries = this.caches.get(name) ?? new Map<string, { body: ArrayBuffer; headers: Headers }>();
        this.caches.set(name, entries);
        return {
            match: async (url: string) => {
                const entry = entries.get(url);
                return entry && new Response(entry.body.slice(0), { headers: entry.headers });
            },
            put: async (url: string, response: Response) => void entries.set(url, { body: await response.arrayBuffer(), headers: response.headers }),
            delete: async (url: string) => entries.delete(url),
            keys: async () => [...entries.keys()].map(url => ({ url })),
        };
    }

    async keys() {
        return [...this.caches.keys()];
    }

    async delete(name: string) {
        return this.caches.delete(name);
    }
}

describe('the device cache of built cells', () => {
    afterEach(() => vi.unstubAllGlobals());
    const WASM = new Uint8Array(readFileSync(new URL('../plan/pkg/obc_ground_plan_bridge_bg.wasm', import.meta.url)));

    /** An engine's records store on `wasm` and `release`, its block uploads and meshes, with its two workers ready. */
    async function engine(wasm: Uint8Array<ArrayBuffer>, release: string) {
        FakeWorker.all = [];
        vi.stubGlobal('Worker', FakeWorker);
        vi.stubGlobal('fetch', async () => new Response(wasm));
        const meshes: ArrayBuffer[] = [];
        const records = new Records(async () => new ArrayBuffer(4), async () => undefined, () => {}, new AbortController().signal, mesh => meshes.push(mesh.bytes), () => {}, release);
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

    /** Frames about B, a mesh cell; the workers answer each build and mesh with bytes of the cell. Returns the builds. */
    async function load({ records }: Awaited<ReturnType<typeof engine>>) {
        let builds = 0;
        for (let frame = 0; frame < 8; frame++) {
            records.update([{ node: B, distance: 0 }], [{ node: B, distance: 0 }]);
            for (let i = 0; i < 5; i++) await settle();
            for (const worker of FakeWorker.all) {
                for (const { id, cell } of worker.builds.splice(0)) {
                    builds++;
                    const words = Uint32Array.from({ length: MODEL_ROW + 7 }, (_, i) => cell.x * 31 + cell.y * 17 + i);
                    // No labels, so no mouths.
                    words[2] = 0;
                    worker.onmessage?.({ data: { id, ok: true, words, rim: new Uint32Array([cell.x, cell.y]), ms: 1, heap: 2 ** 20 } });
                }
                for (const { id, rims } of worker.meshes.splice(0)) worker.onmessage?.({ data: { id, ok: true, mesh: new Uint32Array([...rims, 0x4f424d03]).buffer, ms: 1 } });
            }
        }
        for (let i = 0; i < 20; i++) await settle();
        expect(records.settled(B)).toBe(true);
        return builds;
    }

    it('gives a cell and its mesh as they were built, and misses with another wasm or release', async () => {
        vi.stubGlobal('caches', new FakeCaches());
        const first = await engine(WASM, 'release a');
        expect(await load(first)).toBe(9);
        const again = await engine(WASM, 'release a');
        expect(await load(again)).toBe(0);
        // Blocks take rows in the order that they arrive.
        const blocks = (side: typeof first) => side.blocks().map(words => words.join()).sort();
        expect(again.records.stats.hits).toBe(9);
        expect(again.meshes).toEqual(first.meshes);
        expect(blocks(again)).toEqual(blocks(first));
        // A custom section changes the wasm's bytes, as a change of the crate or its rule tables does.
        const other = await engine(new Uint8Array([...WASM, 0, 2, 1, 0x78]), 'release a');
        expect(await load(other)).toBe(9);
        expect(await load(await engine(WASM, 'release b'))).toBe(9);
    });

    it('drops a corrupt or partial entry, and the least recently used past the cap', async () => {
        vi.stubGlobal('caches', new FakeCaches());
        const cache = (await CellCache.open('release', 'build', 3000))!;
        const words = Uint32Array.from({ length: 300 }, (_, i) => i);
        for (const x of [1, 2]) await cache.write({ x, y: 0 }, 'cell', [words, new Uint32Array([x])]);
        expect((await cache.read({ x: 1, y: 0 }, 'cell'))?.map(part => [...part])).toEqual([[...words], [1]]);
        await cache.write({ x: 3, y: 0 }, 'cell', [words, new Uint32Array([3])]);
        // Three entries of a header, a digest and 301 words do not fit 3000 bytes: the second, used least recently, went.
        expect([await cache.read({ x: 2, y: 0 }, 'cell'), cache.bytes]).toEqual([undefined, 2 * 4 * (4 + 8 + 301)]);
        const [name] = await caches.keys(), entries = (globalThis.caches as unknown as FakeCaches).caches.get(name)!;
        const corrupt = entries.get('https://cells.invalid/1/0/cell')!, partial = entries.get('https://cells.invalid/3/0/cell')!;
        new Uint32Array(corrupt.body)[20] ^= 1;
        partial.body = partial.body.slice(0, 400);
        expect([await cache.read({ x: 1, y: 0 }, 'cell'), await cache.read({ x: 3, y: 0 }, 'cell'), entries.size, cache.bytes]).toEqual([undefined, undefined, 0, 0]);
    });
});
