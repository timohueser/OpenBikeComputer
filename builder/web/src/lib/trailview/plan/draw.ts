import type { Gpu } from '../gpu/device';
import { layouts } from '../gpu/layouts';
import { wgsl } from '../gpu/wgsl';
import { DEBUG_WORDS, type DebugMesh } from './debug';

/** A vertex buffer grows by this when a set of meshes does not fit. */
const GROWTH = 1.5;

interface Resources {
    gpu: Gpu;
    lines: GPURenderPipeline;
    fills: GPURenderPipeline;
    layout: GPUBindGroupLayout;
    depthLayout: GPUBindGroupLayout;
    frame: GPUBuffer;
    ratio: GPUBuffer;
    group: GPUBindGroup;
}

/**
 * The debug draw of the ground plan (plan/debug.wgsl): its own render pass on the canvas after the
 * resolve, so the plan shows in the debug tool's colours at the canvas resolution and the TAA does
 * not touch it. A hill hides what lies behind it; no house or tree hides the plan from above.
 */
export class PlanDraw {
    private resources?: Resources;
    private meshes: DebugMesh[] = [];
    private fresh = true;
    private buffers: Partial<Record<'lines' | 'fills', GPUBuffer>> = {};
    private counts = { lines: 0, fills: 0 };

    /** The vertices of the meshes drawn. */
    get vertices(): number {
        return this.meshes.reduce((sum, { lines, fills }) => sum + (lines.length + fills.length) / DEBUG_WORDS, 0);
    }

    set(meshes: DebugMesh[]): void {
        this.meshes = meshes;
        this.fresh = true;
    }

    /** `frame` is the frame uniform of the eye's view. */
    async attach(gpu: Gpu, frame: GPUBuffer): Promise<void> {
        this.resources = undefined;
        const { lines, fills, layout, depthLayout } = await gpu.once(planPipelines);
        const ratio = gpu.buffer('plan', 16, GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST);
        const group = gpu.device.createBindGroup({ layout, entries: [{ binding: 0, resource: { buffer: frame } }, { binding: 1, resource: { buffer: ratio } }] });
        this.buffers = {};
        this.fresh = true;
        this.resources = { gpu, lines, fills, layout, depthLayout, frame, ratio, group };
    }

    /**
     * Draws the meshes over `canvas`. `depth` holds the scene's depth in its top left, `ratio` render
     * pixels per canvas pixel along each axis.
     */
    encode(encoder: GPUCommandEncoder, canvas: GPUTextureView, depth: GPUTexture, ratio: [number, number], surface: GPUBindGroup, features: GPUBindGroup): void {
        const r = this.resources;
        if (!r) return;
        if (this.fresh) this.write(r);
        if (!this.counts.lines && !this.counts.fills) return;
        r.gpu.device.queue.writeBuffer(r.ratio, 0, new Float32Array([...ratio, 0, 0]));
        const depthGroup = r.gpu.device.createBindGroup({ layout: r.depthLayout, entries: [{ binding: 0, resource: depth.createView() }] });
        const pass = encoder.beginRenderPass({ label: 'plan', colorAttachments: [{ view: canvas, loadOp: 'load', storeOp: 'store' }] });
        [r.group, surface, depthGroup, features].forEach((group, k) => pass.setBindGroup(k, group));
        for (const [kind, pipeline] of [['fills', r.fills], ['lines', r.lines]] as const) {
            if (!this.counts[kind]) continue;
            pass.setPipeline(pipeline);
            pass.setVertexBuffer(0, this.buffers[kind]!);
            pass.draw(this.counts[kind]);
        }
        pass.end();
    }

    private write(r: Resources): void {
        this.fresh = false;
        for (const kind of ['lines', 'fills'] as const) {
            const words = this.meshes.reduce((sum, mesh) => sum + mesh[kind].length, 0);
            this.counts[kind] = words / DEBUG_WORDS;
            if (!words) continue;
            let buffer = this.buffers[kind];
            if (!buffer || buffer.size < 4 * words) {
                if (buffer) r.gpu.destroy('plan', buffer);
                buffer = this.buffers[kind] = r.gpu.buffer('plan', Math.ceil(4 * words * GROWTH / 4) * 4, GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST);
            }
            let offset = 0;
            for (const mesh of this.meshes) {
                if (!mesh[kind].length) continue;
                r.gpu.device.queue.writeBuffer(buffer, offset, mesh[kind]);
                offset += mesh[kind].byteLength;
            }
        }
    }
}

async function planPipelines(gpu: Gpu) {
    const { device } = gpu, { surface, features } = gpu.once(layouts), { VERTEX, FRAGMENT } = GPUShaderStage;
    const layout = device.createBindGroupLayout({ entries: [
        { binding: 0, visibility: VERTEX | FRAGMENT, buffer: { type: 'uniform' } },
        { binding: 1, visibility: FRAGMENT, buffer: { type: 'uniform' } },
    ] });
    const depthLayout = device.createBindGroupLayout({ entries: [{ binding: 0, visibility: FRAGMENT, texture: { sampleType: 'depth' } }] });
    const module = device.createShaderModule({ label: 'plan', code: wgsl('plan/debug.wgsl') });
    const pipelineLayout = device.createPipelineLayout({ bindGroupLayouts: [layout, surface, depthLayout, features] });
    // Premultiplied: a fill covers by its alpha.
    const blend: GPUBlendComponent = { srcFactor: 'one', dstFactor: 'one-minus-src-alpha' };
    const pipeline = (topology: GPUPrimitiveTopology) => device.createRenderPipelineAsync({
        label: `plan ${topology}`, layout: pipelineLayout,
        vertex: {
            module, entryPoint: 'plan_vertex',
            buffers: [{ arrayStride: 4 * DEBUG_WORDS, attributes: [{ shaderLocation: 0, offset: 0, format: 'sint32x2' }, { shaderLocation: 1, offset: 8, format: 'unorm8x4' }] }],
        },
        fragment: { module, entryPoint: 'plan_fragment', targets: [{ format: gpu.format, blend: { color: blend, alpha: blend } }] },
        primitive: { topology },
    });
    const [lines, fills] = await Promise.all([pipeline('line-list'), pipeline('triangle-list')]);
    return { lines, fills, layout, depthLayout };
}
