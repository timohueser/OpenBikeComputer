import { NEAR } from '../frame/camera';
import { CURVATURE } from '../frame/frame';
import type { Gpu } from '../gpu/device';
import { layouts } from '../gpu/layouts';
import { wgsl } from '../gpu/wgsl';
import { HISTORY, HISTORY_DEPTH, type Targets } from './targets';

/** `Resolve` and `Present` in resolve.wgsl. */
const UNIFORM_BYTES = 128, PRESENT_BYTES = 128;
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

/** What the present of a display frame needs: the view projection of the last frame without its jitter, and the view ray of clip point (x, y), forward + x right + y up. */
export interface Present {
    last: Float32Array;
    forward: number[];
    right: number[];
    up: number[];
}

interface Resources {
    gpu: Gpu;
    pipeline: GPURenderPipeline;
    /** With the wide frames: the resolve into the history only, and the present into the canvas. */
    historyOnly?: GPURenderPipeline;
    present?: GPURenderPipeline;
    presentUniform?: GPUBuffer;
    layout: GPUBindGroupLayout;
    uniform: GPUBuffer;
    sampler: GPUSampler;
    nearest: GPUSampler;
    empty: GPUBindGroup;
}

/** The resolve pass: TAA, the sharpening and `finish()`, from the frame to the canvas and the history. */
export class Resolve {
    private resources?: Resources;
    /** Per history that the frame reads. */
    private groups = new Map<GPUTexture, GPUBindGroup>();
    private version = -1;
    private last?: Frame;

    /** `wide`: the targets have wide frames, which present to the canvas. */
    async attach(gpu: Gpu, wide = false): Promise<void> {
        const { device } = gpu, [pipeline, historyOnly, present] = await Promise.all([gpu.once(resolvePipeline), wide ? gpu.once(historyPipeline) : undefined, wide ? gpu.once(presentPipeline) : undefined]);
        const layout = pipeline.getBindGroupLayout(0), empty = pipeline.getBindGroupLayout(1);
        this.resources = {
            gpu, pipeline, layout, empty: device.createBindGroup({ layout: empty, entries: [] }), historyOnly, present,
            presentUniform: present && gpu.buffer('uniforms', PRESENT_BYTES, GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST),
            uniform: gpu.buffer('uniforms', UNIFORM_BYTES, GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST),
            sampler: device.createSampler({ magFilter: 'linear', minFilter: 'linear' }),
            nearest: device.createSampler(),
        };
        this.version = -1;
    }

    /**
     * Encodes the resolve of this frame into the history of the targets' parity, wide or not, and into `canvas`. A wide
     * frame has no canvas: `present` draws it.
     */
    encode(encoder: GPUCommandEncoder, targets: Targets, canvas: GPUTextureView | undefined, look: GPUBindGroup, frame: Frame, blend: Blend, timestampWrites?: GPURenderPassTimestampWrites): void {
        const r = this.resources;
        if (!r || !targets.scene || !targets.depth) return;
        if (this.version !== targets.version) {
            this.version = targets.version;
            this.last = undefined;
            this.groups.clear();
        }
        const { scene, depth } = targets, [lastHistory, lastDepth] = targets.last(), [history, historyDepth] = targets.written(!canvas);
        let group = this.groups.get(lastHistory);
        if (!group) {
            group = r.gpu.device.createBindGroup({ layout: r.layout, entries: [
                { buffer: r.uniform }, scene.createView(), depth.createView(), lastHistory.createView(), r.sampler, r.nearest, lastDepth.createView(),
            ].map((resource, binding) => ({ binding, resource })) });
            this.groups.set(lastHistory, group);
        }
        const last = this.last ?? frame, data = new Float32Array(UNIFORM_BYTES / 4);
        data.set(last.viewProj, 0);
        data.set([frame.eye[0] - last.eye[0], frame.eye[1] - last.eye[1], frame.eye[2] - last.eye[2], CURVATURE], 16);
        data.set([...frame.render, history.width, history.height], 20);
        data.set([...frame.jitter, NEAR, SHARPEN], 24);
        data.set([this.last ? blend.weight : 1, blend.clip ? 1 : 0, blend.copy ? 1 : 0, 0], 28);
        r.gpu.device.queue.writeBuffer(r.uniform, 0, data);
        const pass = encoder.beginRenderPass({
            colorAttachments: [canvas, history.createView(), historyDepth.createView()].map(view => view && ({ view, loadOp: 'clear', storeOp: 'store', clearValue: [0, 0, 0, 1] })),
            timestampWrites,
        });
        pass.setPipeline(canvas ? r.pipeline : r.historyOnly!);
        pass.setBindGroup(0, group);
        pass.setBindGroup(1, r.empty);
        pass.setBindGroup(2, look);
        pass.draw(3);
        pass.end();
        this.last = frame;
    }

    /** Encodes the present: the history of the last frame, turned to the view of `present`, sharpened and finished into `canvas`. */
    present(encoder: GPUCommandEncoder, targets: Targets, canvas: GPUTextureView, look: GPUBindGroup, present: Present, timestampWrites?: GPURenderPassTimestampWrites): void {
        const r = this.resources;
        if (!r?.present || !r.presentUniform) return;
        const [history] = targets.last(), data = new Float32Array(PRESENT_BYTES / 4);
        data.set(present.last, 0);
        data.set([...present.forward, 0, ...present.right, 0, ...present.up, 0, targets.width, targets.height, SHARPEN, 0], 16);
        r.gpu.device.queue.writeBuffer(r.presentUniform, 0, data);
        const group = r.gpu.device.createBindGroup({ layout: r.present.getBindGroupLayout(0), entries: [
            { binding: 3, resource: history.createView() }, { binding: 4, resource: r.sampler }, { binding: 7, resource: { buffer: r.presentUniform } },
        ] });
        const pass = encoder.beginRenderPass({ colorAttachments: [{ view: canvas, loadOp: 'clear', storeOp: 'store', clearValue: [0, 0, 0, 1] }], timestampWrites });
        pass.setPipeline(r.present);
        pass.setBindGroup(0, group);
        pass.setBindGroup(1, r.empty);
        pass.setBindGroup(2, look);
        pass.draw(3);
        pass.end();
    }
}

/** The pass, with the look of shade.wgsl in group 2. */
export function resolvePipeline(gpu: Gpu): Promise<GPURenderPipeline> {
    return resolveWith(gpu, true);
}

/** The pass of a wide frame, into the history only. */
function historyPipeline(gpu: Gpu): Promise<GPURenderPipeline> {
    return resolveWith(gpu, false);
}

function resolveWith(gpu: Gpu, canvas: boolean): Promise<GPURenderPipeline> {
    const { device } = gpu, F = GPUShaderStage.FRAGMENT;
    const layout = device.createBindGroupLayout({ entries: [
        { binding: 0, visibility: F, buffer: { type: 'uniform' } },
        { binding: 1, visibility: F, texture: { sampleType: 'unfilterable-float' } },
        { binding: 2, visibility: F, texture: { sampleType: 'depth' } },
        { binding: 3, visibility: F, texture: { sampleType: 'float' } },
        { binding: 4, visibility: F, sampler: { type: 'filtering' } },
        { binding: 5, visibility: F, sampler: { type: 'non-filtering' } },
        { binding: 6, visibility: F, texture: { sampleType: 'float' } },
    ] });
    const empty = device.createBindGroupLayout({ entries: [] });
    const module = device.createShaderModule({ label: 'resolve', code: wgsl('resolve/resolve.wgsl') });
    return device.createRenderPipelineAsync({
        label: 'resolve', layout: device.createPipelineLayout({ bindGroupLayouts: [layout, empty, gpu.once(layouts).ground] }),
        vertex: { module, entryPoint: 'resolve_vertex' },
        fragment: { module, entryPoint: 'resolve_fragment', targets: [canvas ? { format: gpu.format } : null, { format: HISTORY }, { format: HISTORY_DEPTH }] },
    });
}

/** The present of a wide frame's history to the canvas. */
function presentPipeline(gpu: Gpu): Promise<GPURenderPipeline> {
    const { device } = gpu, F = GPUShaderStage.FRAGMENT;
    const layout = device.createBindGroupLayout({ entries: [
        { binding: 3, visibility: F, texture: { sampleType: 'float' } },
        { binding: 4, visibility: F, sampler: { type: 'filtering' } },
        { binding: 7, visibility: F, buffer: { type: 'uniform' } },
    ] });
    const empty = device.createBindGroupLayout({ entries: [] });
    const module = device.createShaderModule({ label: 'present', code: wgsl('resolve/resolve.wgsl') });
    return device.createRenderPipelineAsync({
        label: 'present', layout: device.createPipelineLayout({ bindGroupLayouts: [layout, empty, gpu.once(layouts).ground] }),
        vertex: { module, entryPoint: 'resolve_vertex' },
        fragment: { module, entryPoint: 'present_fragment', targets: [{ format: gpu.format }] },
    });
}
