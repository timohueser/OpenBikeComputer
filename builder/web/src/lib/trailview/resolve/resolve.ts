import { NEAR } from '../frame/camera';
import { CURVATURE } from '../frame/frame';
import type { Gpu } from '../gpu/device';
import { wgsl } from '../gpu/wgsl';
import { HISTORY, type Targets } from './targets';

/** `Resolve` in resolve.wgsl. */
const UNIFORM_BYTES = 128;
/** The share of the difference to its neighbours that the canvas adds to a pixel. */
const SHARPEN = 0.5;

/** What the resolve of a frame needs: the camera without its jitter, the eye in the frame, the render size and the jitter in render pixels. */
export interface Frame {
    viewProj: Float32Array;
    eye: [number, number, number];
    render: [number, number];
    jitter: [number, number];
}

/** How a frame mixes with the history: the weight of the frame, whether the history keeps to the frame's neighbourhood, or a plain copy of the frame. */
export interface Blend {
    weight: number;
    clip: boolean;
    copy: boolean;
}

interface Resources {
    gpu: Gpu;
    pipeline: GPURenderPipeline;
    layout: GPUBindGroupLayout;
    uniform: GPUBuffer;
    sampler: GPUSampler;
    nearest: GPUSampler;
    empty: GPUBindGroup;
}

/** The resolve pass: TAA, the sharpening and `finish()`, from the frame to the canvas and the history. */
export class Resolve {
    private resources?: Resources;
    /** Per parity of the targets. */
    private groups: GPUBindGroup[] = [];
    private version = -1;
    private last?: Frame;

    /** Compiles the pass, with the look of shade.wgsl in group 2. */
    async attach(gpu: Gpu, look: GPUBindGroupLayout): Promise<void> {
        const { device } = gpu, F = GPUShaderStage.FRAGMENT;
        const layout = device.createBindGroupLayout({ entries: [
            { binding: 0, visibility: F, buffer: { type: 'uniform' } },
            { binding: 1, visibility: F, texture: { sampleType: 'unfilterable-float' } },
            { binding: 2, visibility: F, texture: { sampleType: 'depth' } },
            { binding: 3, visibility: F, texture: { sampleType: 'float' } },
            { binding: 4, visibility: F, sampler: { type: 'filtering' } },
            { binding: 5, visibility: F, sampler: { type: 'non-filtering' } },
        ] });
        const empty = device.createBindGroupLayout({ entries: [] });
        const module = device.createShaderModule({ label: 'resolve', code: wgsl('resolve/resolve.wgsl') });
        const pipeline = await device.createRenderPipelineAsync({
            label: 'resolve', layout: device.createPipelineLayout({ bindGroupLayouts: [layout, empty, look] }),
            vertex: { module, entryPoint: 'resolve_vertex' },
            fragment: { module, entryPoint: 'resolve_fragment', targets: [{ format: gpu.format }, { format: HISTORY }] },
        });
        this.resources = {
            gpu, pipeline, layout, empty: device.createBindGroup({ layout: empty, entries: [] }),
            uniform: gpu.buffer('uniforms', UNIFORM_BYTES, GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST),
            sampler: device.createSampler({ magFilter: 'linear', minFilter: 'linear' }),
            nearest: device.createSampler(),
        };
        this.version = -1;
    }

    /** Encodes the resolve of this frame into the canvas and the history of the targets' parity. */
    encode(encoder: GPUCommandEncoder, targets: Targets, canvas: GPUTextureView, look: GPUBindGroup, frame: Frame, blend: Blend, timestampWrites?: GPURenderPassTimestampWrites): void {
        const r = this.resources;
        if (!r || !targets.scene || !targets.depth) return;
        if (this.version !== targets.version) {
            this.version = targets.version;
            this.last = undefined;
            const { scene, depth, history } = targets;
            this.groups = [0, 1].map(p => r.gpu.device.createBindGroup({ layout: r.layout, entries: [
                { buffer: r.uniform }, scene.createView(), depth.createView(), history[1 - p].createView(), r.sampler, r.nearest,
            ].map((resource, binding) => ({ binding, resource })) }));
        }
        const last = this.last ?? frame, data = new Float32Array(UNIFORM_BYTES / 4);
        data.set(last.viewProj, 0);
        data.set([frame.eye[0] - last.eye[0], frame.eye[1] - last.eye[1], frame.eye[2] - last.eye[2], CURVATURE], 16);
        data.set([...frame.render, 0, 0], 20);
        data.set([...frame.jitter, NEAR, SHARPEN], 24);
        data.set([this.last ? blend.weight : 1, blend.clip ? 1 : 0, blend.copy ? 1 : 0, 0], 28);
        r.gpu.device.queue.writeBuffer(r.uniform, 0, data);
        const pass = encoder.beginRenderPass({
            colorAttachments: [canvas, targets.history[targets.parity].createView()].map(view => ({ view, loadOp: 'clear', storeOp: 'store', clearValue: [0, 0, 0, 1] })),
            timestampWrites,
        });
        pass.setPipeline(r.pipeline);
        pass.setBindGroup(0, this.groups[targets.parity]);
        pass.setBindGroup(1, r.empty);
        pass.setBindGroup(2, look);
        pass.draw(3);
        pass.end();
        this.last = frame;
    }
}
