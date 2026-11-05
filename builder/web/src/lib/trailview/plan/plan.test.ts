import { readFileSync } from 'node:fs';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import { worldPixel } from '../../planner/layers/mercator';
import { anchorAt, UNITS } from '../frame/space';
import { distances, size } from '../terrain/lod';
import { DEBUG_WORDS, debugMesh, EDGE_STEP_M, type Vocabulary } from './debug';
import { metresPerUnit } from '../terrain/bins';
import { AHEAD_M, BUILD_M, CELL_LEVEL, Plan, wantedCells, type PlanEye } from './plan';
import type { Built, Failed, PlanRequest } from './plan-worker';
import { build, CellRequest, initSync, vocabulary, wanted } from './pkg/obc_ground_plan_bridge.js';

const [west, north] = worldPixel([8.37, 46.83], UNITS), [east, south] = worldPixel([8.46, 46.765], UNITS);
const BOUNDS: [number, number, number, number] = [west, north, east, south];
const eyeAt = (lon: number, lat: number, above: number): PlanEye => {
    const [x, y] = worldPixel([lon, lat], UNITS);
    return { x, y, unit: anchorAt(lon, lat).unit, above };
};
const metres = (eye: PlanEye, node: { level: number; x: number; y: number }) =>
    distances({ anchor: { whole: [eye.x, eye.y], part: [0, 0], unit: eye.unit }, above: eye.above }, node)[0];

describe('the wanted cells', () => {
    const eye = eyeAt(8.405, 46.818, 1.6), east: [number, number] = [1, 0];

    it('come near first by distance, then those ahead, none past the range and none outside the bounds', () => {
        const cells = wantedCells(eye, east, BOUNDS), d = cells.map(cell => metres(eye, cell));
        const near = d.filter(m => m < BUILD_M).length;
        expect(near).toBeGreaterThan(4);
        expect(cells.length).toBeGreaterThan(near);
        expect(d.slice(0, near)).toEqual([...d.slice(0, near)].sort((a, b) => a - b));
        expect(d.slice(near).every(m => m >= BUILD_M && m < BUILD_M + AHEAD_M)).toBe(true);
        const s = size(CELL_LEVEL);
        for (const cell of cells.slice(near)) expect((cell.x + 0.5) * s - eye.x).toBeGreaterThan(0);
        expect(wantedCells(eye, undefined, BOUNDS)).toEqual(cells.slice(0, near));
        // At the west edge of the bounds, no cell lies west of it.
        const edge = eyeAt(8.3705, 46.8, 1.6);
        expect(wantedCells(edge, [-1, 0], BOUNDS).every(({ x }) => (x + 1) * s > BOUNDS[0])).toBe(true);
        expect(wantedCells(eyeAt(8.405, 46.818, 700), east, BOUNDS)).toEqual([]);
    });
});

describe('the bridge', () => {
    let words: Vocabulary;
    beforeAll(() => {
        initSync({ module: readFileSync(new URL('./pkg/obc_ground_plan_bridge_bg.wasm', import.meta.url)) });
        words = JSON.parse(vocabulary());
    });
    // The north-west cell of zoom-13 tile 4286/2888, the tile of the vector: its roads with heights lie there.
    const cell = { x: 4286 * 8, y: 2888 * 8 };

    it('names pages of levels 13 and 14, and builds a cell from a feature tile alone, twice the same', () => {
        const pages = wanted(cell.x, cell.y);
        expect(pages.length).toBeGreaterThan(0);
        for (let i = 0; i < pages.length; i += 7) expect([13, 14]).toContain(pages[i]);
        const plan = () => {
            const request = new CellRequest(cell.x, cell.y);
            request.add_tile(4286, 2888, readFileSync(new URL('../../../../../../specs/vectors/trailview-features.bin', import.meta.url)));
            try {
                return build(request);
            } finally {
                request.free();
            }
        };
        const [a, b] = [plan(), plan()];
        try {
            expect(JSON.parse(a.stats()).inputs).toMatchObject({ pages: 0, tiles: 1 });
            const samples = a.samples().length / 2, ways = a.ways();
            expect(ways.length).toBeGreaterThan(0);
            for (let w = 0; w < ways.length; w += 6) expect(ways[w + 4] + ways[w + 5]).toBeLessThanOrEqual(samples);
            expect(a.digest()).toBe(b.digest());
            expect(a.times().length).toBe(words.stages.length);
            // The debug mesh of the real plan: every vertex in the cell square, every segment within a step.
            const vertices = a.vertices().length / 2, rings = a.rings(), ringVertices = a.ring_vertices();
            expect(a.faces().length).toBeGreaterThan(0);
            for (let r = 0; r < rings.length; r += 2) expect(rings[r] + rings[r + 1]).toBeLessThanOrEqual(ringVertices.length);
            expect(ringVertices.every(v => v < vertices) && a.edges().every((v, i) => i % 6 > 1 || v < vertices)).toBe(true);
            const mesh = debugMesh({ samples: a.samples(), ways, points: a.points(), crossings: a.crossings(), vertices: a.vertices(), faces: a.faces(), rings, ring_vertices: ringVertices, edges: a.edges() }, words, cell);
            expect(mesh.fills.length).toBeGreaterThan(0);
            expect(mesh.unfilled).toBe(0);
            const s = size(CELL_LEVEL), unit = metresPerUnit((cell.y + 0.5) * s);
            expect(mesh.lines.length).toBeGreaterThan(0);
            for (let v = 0; v < mesh.lines.length; v += 2 * DEBUG_WORDS) {
                const [ax, ay, , bx, by] = mesh.lines.subarray(v, v + 2 * DEBUG_WORDS);
                for (const [x, y] of [[ax, ay], [bx, by]]) {
                    expect(x >= cell.x * s && x <= (cell.x + 1) * s && y >= cell.y * s && y <= (cell.y + 1) * s).toBe(true);
                }
                expect(Math.hypot(bx - ax, by - ay) * unit).toBeLessThanOrEqual(EDGE_STEP_M + 0.1);
            }
        } finally {
            a.free();
            b.free();
        }
    });
});

/** A plan worker that answers `start` and keeps the build messages for the test to answer. */
class FakeWorker {
    static all: FakeWorker[] = [];
    onmessage?: (event: { data: unknown }) => void;
    onerror?: unknown;
    builds: Extract<PlanRequest, { type: 'build' }>[] = [];
    terminated = false;

    constructor() {
        FakeWorker.all.push(this);
    }

    postMessage(message: PlanRequest): void {
        if (message.type === 'start') queueMicrotask(() => this.onmessage?.({ data: { type: 'ready' } }));
        if (message.type === 'build') this.builds.push(message);
    }

    answer(build: { id: number }, reply: Partial<Built> | Partial<Failed>): void {
        this.onmessage?.({ data: { id: build.id, ...reply } });
    }

    terminate(): void {
        this.terminated = true;
    }
}

describe('the plan', () => {
    const settle = () => new Promise(resolve => setTimeout(resolve, 0));
    const eye = eyeAt(8.405, 46.818, 1.6);
    const built = (bytes: number, heap = 2 ** 20): Partial<Built> => ({
        ok: true, stats: JSON.stringify({ inputs: { request: 'r' } }), times: [1, 2], prepare: 3, heap,
        debug: { lines: new Int32Array(bytes / 4), fills: new Int32Array(0), unfilled: 0 },
    });
    const start = async () => {
        FakeWorker.all = [];
        vi.stubGlobal('Worker', FakeWorker);
        vi.stubGlobal('fetch', async () => new Response(readFileSync(new URL('./pkg/obc_ground_plan_bridge_bg.wasm', import.meta.url))));
        const plan = new Plan(async () => new ArrayBuffer(4), async () => undefined, BOUNDS, () => {}, new AbortController().signal);
        plan.debug = true;
        for (let i = 0; i < 20 && !plan.vocabulary; i++) await settle();
        await settle();
        return plan;
    };
    afterEach(() => vi.unstubAllGlobals());

    it('keeps the records inside their budget and asks no cell again that would not fit', async () => {
        const plan = await start(), worker = FakeWorker.all[0], asked = new Map<string, number>();
        // Debug meshes of 12 MiB: two of them fit in the 32 MiB of the records.
        for (let frame = 0; frame < 30; frame++) {
            plan.update(eye, undefined, true);
            await settle();
            for (const build of worker.builds.splice(0)) {
                const name = `${build.cell.x}-${build.cell.y}`;
                asked.set(name, (asked.get(name) ?? 0) + 1);
                worker.answer(build, built(12 * 2 ** 20));
            }
        }
        // Two fit; the two jobs in flight when they arrived built once each and went.
        expect(asked.size).toBeGreaterThan(2);
        expect(asked.size).toBeLessThanOrEqual(4);
        expect([...asked.values()].every(n => n === 1)).toBe(true);
        expect(plan.stats.cells).toBe(2);
        expect(plan.loading).toBe(false);
        expect(plan.stats.build.prepare).toBe(3);
    });

    it('starts a new worker after a trap or a full heap, also for a cell that went out of range', async () => {
        const plan = await start();
        plan.update(eye, undefined, true);
        await settle();
        const [first, second] = FakeWorker.all[0].builds;
        FakeWorker.all[0].answer(first, { ok: false, error: 'unreachable', trapped: true });
        expect(FakeWorker.all.length).toBe(2);
        expect(FakeWorker.all[0].terminated).toBe(true);
        expect(plan.stats).toMatchObject({ restarts: 1, failed: 1 });
        // A late answer of the ended worker counts no more.
        FakeWorker.all[0].answer(second, built(4));
        await settle();
        plan.update(eye, undefined, true);
        await settle();
        const next = FakeWorker.all[1].builds[0];
        plan.update(eyeAt(8.445, 46.818, 1.6), undefined, true);
        FakeWorker.all[1].answer(next, built(4, 60 * 2 ** 20));
        expect(FakeWorker.all.length).toBe(3);
        expect(plan.stats.restarts).toBe(2);
    });
});
