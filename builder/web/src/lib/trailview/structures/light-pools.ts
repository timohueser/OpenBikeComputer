import type { Gpu } from '../gpu/device';
import { layouts } from '../gpu/layouts';
import { wgsl } from '../gpu/wgsl';
import { sceneFormat } from '../resolve/targets';

/** Words of a pool, as `Pool` of light-pools.wgsl: x, y (int32 world units), the colour as sRGB bytes and the radius in metres. */
export const POOL_WORDS = 4;
/** A pool draws a square of two triangles over its light, as pool_vertex of light-pools.wgsl. */
const VERTICES = 6;

/** The light that a kind's model sheds on the ground about it at night. */
export interface Light { colour: string; radius: number }

/** The words of a pool of `light` at world point x, y. */
export function poolWords(x: number, y: number, { colour, radius }: Light): number[] {
    const f = new Float32Array([radius]);
    return [x | 0, y | 0, parseInt(colour.slice(5, 7) + colour.slice(3, 5) + colour.slice(1, 3), 16), new Uint32Array(f.buffer)[0]];
}

interface Resources { gpu: Gpu; pipeline: GPURenderPipeline; layout: GPUBindGroupLayout; frame: GPUBuffer; size: GPUBuffer; pools?: GPUBuffer }

/** Pools of light about street lamps and fires at night, after the frame, with light-pools.wgsl. */
export class LightPools {
    newContent = false;
    private words = new Uint32Array(0);
    private fresh = true;
    private resources?: Resources;

    /** The pools, POOL_WORDS each; a new array replaces them. */
    set(words: Uint32Array<ArrayBuffer>): void {
        if (words === this.words) return;
        this.words = words;
        this.fresh = this.newContent = true;
    }

    /** `frame` is the frame uniform of the eye's view. */
    async attach(gpu: Gpu, frame: GPUBuffer): Promise<void> {
        this.resources = undefined;
        const { pipeline, layout } = await gpu.once(poolPipeline);
        this.resources = { gpu, pipeline, layout, frame, size: gpu.buffer('pools', 16, GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST) };
        this.fresh = true;
    }

    /** Adds the pools' light to `scene`, whose frame of `size` pixels in its top left has `depth`. */
    encode(encoder: GPUCommandEncoder, scene: GPUTexture, depth: GPUTexture, [width, height]: [number, number], surface: GPUBindGroup, ground: GPUBindGroup, features: GPUBindGroup): void {
        const r = this.resources;
        this.newContent = false;
        if (!r) return;
        const { device } = r.gpu;
        if (this.fresh) {
            this.fresh = false;
            if (this.words.length && (!r.pools || r.pools.size < this.words.byteLength)) {
                if (r.pools) r.gpu.destroy('pools', r.pools);
                r.pools = r.gpu.buffer('pools', this.words.byteLength, GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST);
            }
            if (this.words.length) device.queue.writeBuffer(r.pools!, 0, this.words);
        }
        if (!r.pools || !this.words.length) return;
        device.queue.writeBuffer(r.size, 0, new Float32Array([width, height, 0, 0]));
        const group = device.createBindGroup({ layout: r.layout, entries: [
            { binding: 0, resource: { buffer: r.frame } }, { binding: 1, resource: { buffer: r.pools } },
            { binding: 2, resource: depth.createView() }, { binding: 3, resource: { buffer: r.size } },
        ] });
        const pass = encoder.beginRenderPass({ label: 'light pools', colorAttachments: [{ view: scene.createView(), loadOp: 'load', storeOp: 'store' }] });
        pass.setViewport(0, 0, width, height, 0, 1);
        pass.setPipeline(r.pipeline);
        [group, surface, ground, features].forEach((g, k) => pass.setBindGroup(k, g));
        pass.draw(VERTICES, this.words.length / POOL_WORDS);
        pass.end();
    }
}

async function poolPipeline(gpu: Gpu) {
    const { device } = gpu, { surface, ground, features } = gpu.once(layouts), { VERTEX, FRAGMENT } = GPUShaderStage;
    const layout = device.createBindGroupLayout({ entries: [
        { binding: 0, visibility: VERTEX, buffer: { type: 'uniform' } },
        { binding: 1, visibility: VERTEX, buffer: { type: 'read-only-storage' } },
        { binding: 2, visibility: FRAGMENT, texture: { sampleType: 'depth' } },
        { binding: 3, visibility: FRAGMENT, buffer: { type: 'uniform' } },
    ] });
    const module = device.createShaderModule({ label: 'light pools', code: wgsl('structures/light-pools.wgsl') });
    // Added to the frame: light falls on what lies there.
    const add: GPUBlendComponent = { srcFactor: 'one', dstFactor: 'one' };
    const pipeline = await device.createRenderPipelineAsync({
        label: 'light pools', layout: device.createPipelineLayout({ bindGroupLayouts: [layout, surface, ground, features] }),
        vertex: { module, entryPoint: 'pool_vertex' },
        fragment: { module, entryPoint: 'pool_fragment', targets: [{ format: sceneFormat(gpu), blend: { color: add, alpha: add } }] },
    });
    return { pipeline, layout };
}
