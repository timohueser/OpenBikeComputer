import { readFileSync } from 'node:fs';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { CELL_LEVEL, MODEL_ROW, Records } from './model';
import type { PlanRequest } from '../plan/plan-worker';

/** A plan worker that answers `start` and keeps the cell builds for the test to answer. */
class FakeWorker {
    static all: FakeWorker[] = [];
    onmessage?: (event: { data: unknown }) => void;
    onerror?: unknown;
    builds: Extract<PlanRequest, { type: 'cell' }>[] = [];
    meshes: Extract<PlanRequest, { type: 'mesh' }>[] = [];

    constructor() {
        FakeWorker.all.push(this);
    }

    postMessage(message: PlanRequest): void {
        if (message.type === 'start') queueMicrotask(() => this.onmessage?.({ data: { type: 'ready' } }));
        else if (message.type === 'cell') this.builds.push(message);
        else if (message.type === 'mesh') this.meshes.push(message);
    }

    /** Answers a build with a block of `rows` rows and a rim. */
    answer(build: { id: number }, rows: number, rim = new Uint32Array(0)): void {
        this.onmessage?.({ data: { id: build.id, ok: true, words: new Uint32Array(rows * MODEL_ROW), rim, ms: 1, heap: 2 ** 20 } });
    }

    terminate(): void {}
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
});
