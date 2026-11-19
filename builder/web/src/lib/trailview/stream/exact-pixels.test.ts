import { describe, expect, it, vi } from 'vitest';

const browser = vi.fn(), wasm = vi.fn();
vi.mock('../../planner/layers/terrain', () => ({ imagePixels: (body: Uint8Array) => browser(body) }));
vi.mock('./webp/obc_webp_bridge.js', () => ({ default: async () => {}, decode_webp: (body: Uint8Array, size: Uint32Array) => wasm(body, size) }));

const { terrainPixels } = await import('./exact-pixels');
const tile = (...alphas: number[]) => ({ width: alphas.length, data: Uint8ClampedArray.from(alphas.flatMap(a => [1, 2, 3, a])) });

describe('terrainPixels', () => {
    it('keeps the browser decode of a tile with alpha 0 and 255 only, and decodes in WASM from the first tile with another alpha', async () => {
        wasm.mockImplementation((_: Uint8Array, size: Uint32Array) => ((size[0] = 2), new Uint8Array([1, 2, 3, 255, 1, 2, 3, 0])));
        browser.mockResolvedValueOnce(tile(255, 0));
        expect(await terrainPixels(new ArrayBuffer(1))).toEqual({ size: 2, data: new Uint8Array([1, 2, 3, 255, 1, 2, 3, 0]) });
        expect(wasm).not.toHaveBeenCalled();
        browser.mockResolvedValueOnce(tile(255, 254));
        expect(await terrainPixels(new ArrayBuffer(1))).toEqual({ size: 2, data: new Uint8Array([1, 2, 3, 255, 1, 2, 3, 0]) });
        // Later tiles of the worker go to WASM without the browser's decode.
        await terrainPixels(new ArrayBuffer(1));
        expect(browser).toHaveBeenCalledTimes(2);
        expect(wasm).toHaveBeenCalledTimes(2);
    });
});
