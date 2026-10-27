import type { Gpu } from '../gpu/device';
import { layouts } from '../gpu/layouts';
import { wgsl } from '../gpu/wgsl';
import { f16 } from '../instances/points';
import { DEPTH, sceneFormat } from '../resolve/targets';
import { metresPerUnit } from '../terrain/bins';

/**
 * Words of a pool, as `Pool` of light-pools.wgsl: x, y of its source (int32 world units), the colour as
 * sRGB bytes, and the radius and the source's height over the ground in metres as two float16.
 */
export const POOL_WORDS = 4;
/** A pool draws the 12 triangles of a box about its light, as pool_vertex of light-pools.wgsl. */
const VERTICES = 36;

/**
 * The light that a kind's model sheds about it at night: it reaches `radius` metres from its source, which
 * stands at `at` in the model's metres from its foot, x right, y forward (−y the front) and z up.
 */
export interface Light { colour: string; radius: number; at: [number, number, number] }

/** The words of a pool of `light` of a model at world point x, y, turned by `turn` 256ths of a turn as points.wgsl turns it. */
export function poolWords(x: number, y: number, turn: number, { colour, radius, at: [right, ahead, up] }: Light): number[] {
    const yaw = turn / 256 * 2 * Math.PI, cos = Math.cos(yaw), sin = Math.sin(yaw), unit = metresPerUnit(y);
    // East and north in metres, as draw.wgsl turns a model; world y runs south.
    const east = cos * right - sin * ahead, north = sin * right + cos * ahead;
    return [Math.round(x + east / unit), Math.round(y - north / unit), parseInt(colour.slice(5, 7) + colour.slice(3, 5) + colour.slice(1, 3), 16), f16(radius) | (f16(up) << 16)];
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
        const pass = encoder.beginRenderPass({
            label: 'light pools', colorAttachments: [{ view: scene.createView(), loadOp: 'load', storeOp: 'store' }],
            depthStencilAttachment: { view: depth.createView(), depthReadOnly: true },
        });
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
        // The back faces of a light's box draw where the frame lies before them: each pixel whose ray
        // crosses the box once, and none where the frame lies past the box (reversed Z).
        primitive: { cullMode: 'front' },
        depthStencil: { format: DEPTH, depthWriteEnabled: false, depthCompare: 'less-equal' },
    });
    return { pipeline, layout };
}
