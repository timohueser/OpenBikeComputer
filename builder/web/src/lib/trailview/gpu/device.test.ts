import { afterEach, expect, it, vi } from 'vitest';
import { Gpu } from './device';

afterEach(() => vi.unstubAllGlobals());

it('releases the buffers and textures of a scope only, and its pools drop back', async () => {
    const destroyed: number[] = [];
    let next = 0;
    const make = () => { const id = next++; return { destroy: () => destroyed.push(id) }; };
    const device = { createBuffer: make, createTexture: make, addEventListener: () => {} };
    vi.stubGlobal('navigator', { gpu: { requestAdapter: async () => ({ features: new Set(), requestDevice: async () => device }), getPreferredCanvasFormat: () => 'bgra8unorm' } });
    const gpu = (await Gpu.request())!;
    gpu.buffer('uniforms', 64, 0);
    const scope = gpu.scope();
    scope.buffer('uniforms', 256, 0);
    scope.texture('pages', {} as GPUTextureDescriptor, 1024);
    expect(Object.fromEntries(gpu.pools)).toEqual({ uniforms: 320, pages: 1024 });
    scope.release();
    scope.release();
    expect(destroyed).toEqual([1, 2]);
    expect(Object.fromEntries(gpu.pools)).toEqual({ uniforms: 64, pages: 0 });
});
