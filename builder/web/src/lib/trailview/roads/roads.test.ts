import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Gpu } from '../gpu/device';
import { CHILD, size } from '../terrain/lod';
import type { RoadMesh } from './mesh';
import type { RoadPiece } from './piece';
import { Pool } from '../structures/pool';
import { RANGE_M, Roads } from './roads';

/** Metres per world unit about the eye, and the owner that holds it. */
const unit = 0.025, side = size(CHILD);
const owner = { x: 2000, y: 2000 }, eye = { x: (owner.x * 2 + 0.5) * side, y: (owner.y * 2 + 0.5) * side, unit };

/** A piece of child (i, j) of the eye's owner, that takes `bytes` of each pool. The eye stands over the middle of child (0, 0). */
const pieceAt = (i: number, j: number, bytes: number): RoadPiece => ({
    x: (owner.x * 2 + i + 0.5) * side, y: (owner.y * 2 + j + 0.5) * side, low: 0, high: 1, over: 0, vertices: new ArrayBuffer(bytes), indices: new Uint16Array(bytes / 2),
    near: { first: 0, count: 300, vertices: 100 }, far: { first: 300, count: 30, vertices: 10 }, casters: { first: 330, count: 0, vertices: 0 },
});

/** A worker that answers the eye's owner with `mesh`, or with `error`, and each other owner with nothing. */
function workerOf(mesh: RoadMesh, error?: string) {
    return class {
        onmessage?: (event: { data: unknown }) => void;
        postMessage({ id, owner: asked }: { id: number; owner: { x: number; y: number } }) {
            const here = asked.x === owner.x && asked.y === owner.y;
            queueMicrotask(() => this.onmessage?.({ data: here && error ? { id, error } : { id, mesh: here ? mesh : { pieces: [] } } }));
        }
        terminate() {}
    };
}

/** Roads over pools a 64th of their size. */
async function attached(mesh: RoadMesh, error?: string) {
    vi.stubGlobal('Worker', workerOf(mesh, error));
    vi.stubGlobal('GPUBufferUsage', { VERTEX: 1, INDEX: 2, COPY_DST: 4 });
    const gpu = {
        device: { createBindGroup: () => ({}), queue: { writeBuffer: () => {} } },
        once: async () => ({ layout: {}, pipeline: {}, caster: {} }), buffer: (_: string, bytes: number) => ({ size: bytes / 64 }),
    };
    const roads = new Roads(async () => new ArrayBuffer(8), async () => undefined, () => {}, new AbortController().signal);
    await roads.attach(gpu as unknown as Gpu, [{}, {}] as GPUBuffer[]);
    return roads;
}

/** The same, with all the meshes in. */
async function roadsWith(mesh: RoadMesh, error?: string) {
    const roads = await attached(mesh, error);
    // The requests of the first update come back as microtasks, each a mesh of a tile fetch away.
    const settle = async () => {
        for (let round = 0; round < 40 && (roads.update(eye, () => true, true), roads.loading || round < 2); round++) await new Promise(resolve => setTimeout(resolve));
        roads.update(eye, () => true, true);
    };
    await settle();
    return { roads };
}

afterEach(() => void vi.unstubAllGlobals());

describe('the corridors', () => {
    it('ask for no mesh until the first view lets them', () => {
        vi.stubGlobal('Worker', workerOf({ pieces: [] }));
        const roads = new Roads(async () => new ArrayBuffer(8), async () => undefined, () => {}, new AbortController().signal);
        roads.update(eye, () => true, false);
        expect(roads.loading).toBe(false);
        roads.update(eye, () => true, true);
        expect(roads.loading).toBe(true);
    });
});

describe('an owner that fails', () => {
    it('holds no way: what corridors hold stops at it, so the paint stays, and it counts', async () => {
        const warn = vi.spyOn(console, 'warn').mockImplementation(() => {}), { roads } = await roadsWith({ pieces: [] }, 'no mesh');
        expect(roads.write()).toBe(0);
        expect(roads.stats.pieces).toBe(0);
        expect(roads.failed).toBe(1);
        warn.mockRestore();
    });
});

describe('the last owner to arrive', () => {
    it('reports a change with no piece in range, so a still view takes the new reach', async () => {
        const roads = await attached({ pieces: [] });
        let reach = roads.write();
        for (let round = 0; round < 40; round++) {
            const changed = roads.update(eye, () => true, true), now = roads.write();
            if (now !== reach) expect(changed).toBe(true);
            reach = now;
            await new Promise(resolve => setTimeout(resolve));
        }
        expect(reach).toBe(RANGE_M);
    });
});

describe('a mesh that the cache evicts', () => {
    it('leaves the pool too, so its bytes do not outlive the budget', async () => {
        const forget = vi.spyOn(Pool.prototype, 'forget'), big = pieceAt(0, 0, 33 * 2 ** 20), { roads } = await roadsWith({ pieces: [big] });
        roads.update({ ...eye, x: eye.x + 4 * size(CHILD) * 4 }, () => true, false);
        expect(forget).toHaveBeenCalledWith(big);
    });
});

describe('the corridors under pressure', () => {
    it('draw the near range under the eye and the far range past 200 m, and count none dropped with room', async () => {
        const { roads } = await roadsWith({ pieces: [pieceAt(0, 0, 4096), pieceAt(1, 0, 4096), pieceAt(0, 1, 4096), pieceAt(1, 1, 4096)] });
        expect(roads.write()).toBe(RANGE_M);
        expect(roads.stats).toMatchObject({ pieces: 4, near: 1, far: 3, dropped: 0, vertices: 130, triangles: 130 });
    });

    it('lose the far ranges first, count them, and keep the paint of every way past what is left', async () => {
        // The index pool, over 64, holds two pieces of 128 KiB. The nearest stays.
        const big = 128 * 1024, first = pieceAt(0, 0, big), { roads } = await roadsWith({ pieces: [pieceAt(1, 1, big), pieceAt(0, 1, big), first, pieceAt(1, 0, big)] });
        const reach = roads.write();
        expect(roads.stats).toMatchObject({ pieces: 2, near: 1, far: 1, dropped: 2 });
        // The nearest piece that did not fit starts half a child from the eye: no way past it loses its paint.
        expect(reach).toBeCloseTo(side / 2 * unit, 3);
    });
});
