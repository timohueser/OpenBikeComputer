import { describe, expect, it } from 'vitest';
import { Pool, type Mesh } from './pool';

const mesh = (): Mesh => ({ vertices: new ArrayBuffer(16), indices: new Uint32Array(1) });

/** A pool of room for `count` meshes, with its count of uploads. */
function poolOf(count: number) {
    const uploads = { count: 0 }, device = { queue: { writeBuffer: () => uploads.count++ } } as unknown as GPUDevice;
    return { uploads, pool: new Pool(device, { size: 16 * count } as GPUBuffer, { size: 4 * count } as GPUBuffer, 16) };
}

describe('a pool', () => {
    it('gives a mesh that comes early in the frame the room of those after it, whenever it arrives', () => {
        const { pool } = poolOf(2), [near, a, b] = [mesh(), mesh(), mesh()];
        pool.write([a, b]);
        expect(pool.dropped).toBe(0);
        pool.write([near, a, b]);
        expect(pool.at(near)).toBeDefined();
        expect(pool.at(a)).toBeDefined();
        expect(pool.at(b)).toBeUndefined();
        expect(pool.dropped).toBe(1);
    });

    it('starts again once: a frame that does not fit uploads nothing more while it stays the same', () => {
        const { pool, uploads } = poolOf(2), draws = [mesh(), mesh(), mesh()];
        pool.write(draws);
        const after = uploads.count;
        pool.write(draws);
        expect(uploads.count).toBe(after);
        expect(pool.dropped).toBe(1);
    });

    it('keeps the room of a mesh that the frame leaves out until it needs it, and takes it back then', () => {
        const { pool, uploads } = poolOf(2), [a, b, c] = [mesh(), mesh(), mesh()];
        pool.write([a, b]);
        const after = uploads.count;
        pool.write([b]);
        pool.write([a, b]);
        expect(uploads.count).toBe(after);
        pool.write([b, c]);
        expect([pool.at(b), pool.at(c)].every(Boolean)).toBe(true);
        expect(pool.at(a)).toBeUndefined();
    });

    it('lets go of a forgotten mesh, and its room comes back when a mesh needs it', () => {
        const { pool } = poolOf(1), [a, b] = [mesh(), mesh()];
        pool.write([a]);
        pool.forget(a);
        expect(pool.at(a)).toBeUndefined();
        pool.write([b]);
        expect(pool.at(b)).toBeDefined();
        expect(pool.dropped).toBe(0);
    });
});
