import type { Gpu } from '../gpu/device';
import { wgsl } from '../gpu/wgsl';
import { Instances, type Producer, type Surfaces } from './instances';

/** Words of a point, as `Point` in points.wgsl. */
export const POINT_WORDS = 4;

/** A point instance: whole world units (2^30 across the world), a turn in radians, a height in metres, a model and a tint (0.75 to 1.25). */
export interface Point { x: number; y: number; yaw: number; height: number; model: number; tint: number }

/** Points as points.wgsl reads them, POINT_WORDS words each. */
export function packPoints(points: Point[]): Uint32Array<ArrayBuffer> {
    const out = new Uint32Array(points.length * POINT_WORDS), ints = new Int32Array(out.buffer), half = new Uint16Array(out.buffer);
    points.forEach(({ x, y, yaw, height, model, tint }, i) => {
        const turn = Math.round(((yaw / (2 * Math.PI)) % 1 + 1) % 1 * 256) & 255;
        ints.set([Math.floor(x), Math.floor(y)], POINT_WORDS * i);
        out[POINT_WORDS * i + 2] = (model & 0xffff) | (turn << 16) | (Math.round(Math.min(1, Math.max(0, (tint - 0.75) * 2)) * 255) << 24);
        half[2 * (POINT_WORDS * i + 3)] = f16(height);
    });
    return out;
}

/** The bits of a positive number as a float16. */
export function f16(value: number): number {
    const view = new DataView(new ArrayBuffer(4));
    view.setFloat32(0, value);
    const bits = view.getUint32(0), exponent = ((bits >> 23) & 255) - 127 + 15;
    if (exponent <= 0) return 0;
    if (exponent >= 31) return 0x7bff;
    return Math.min(0x7bff, (exponent << 10) + ((bits >> 13) & 0x3ff) + ((bits >> 12) & 1));
}

interface Resources { device: GPUDevice; pipeline: GPUComputePipeline; group: GPUBindGroup; points: GPUBuffer; pool: GPUBuffer; surface: GPUBindGroup; features: GPUBindGroup }

/**
 * A pool of stored point instances, such as the rocks or the herds of a kind: a producer that stands
 * each point on the drawn surface every frame and emits it, so the core culls, picks the tier and
 * draws it. The pool has a fixed size; `write` replaces its points.
 */
export class Points implements Producer {
    private resources?: Resources;
    private count = 0;
    private pending?: Uint32Array<ArrayBuffer>;

    constructor(private readonly instances: Instances, readonly capacity: number) {
        instances.produce(this);
    }

    async attach(gpu: Gpu, frame: GPUBuffer, { surface, features, ground }: Surfaces): Promise<void> {
        this.resources = undefined;
        const { device } = gpu, C = GPUShaderStage.COMPUTE;
        const points = gpu.buffer('instances', Math.max(16, this.capacity * POINT_WORDS * 4), GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST);
        const pool = gpu.buffer('instances', 16, GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST);
        const layout = device.createBindGroupLayout({ entries: [
            ...Instances.emitEntries(), { binding: 8, visibility: C, buffer: { type: 'read-only-storage' } }, { binding: 9, visibility: C, buffer: { type: 'uniform' } },
        ] });
        const pipeline = await device.createComputePipelineAsync({
            label: 'points', layout: device.createPipelineLayout({ bindGroupLayouts: [layout, surface.layout, ground, features.layout] }),
            compute: { module: device.createShaderModule({ label: 'points', code: wgsl('instances/points.wgsl') }), entryPoint: 'stand' },
        });
        this.resources = {
            device, pipeline, points, pool, surface: surface.group, features: features.group,
            group: device.createBindGroup({ layout, entries: [...this.instances.emitResources(frame), { binding: 8, resource: { buffer: points } }, { binding: 9, resource: { buffer: pool } }] }),
        };
        if (this.pending) this.write(this.pending);
    }

    /** Replaces the points of the pool, POINT_WORDS words each (`packPoints`); those past its capacity are left out. */
    write(points: Uint32Array<ArrayBuffer>): void {
        this.pending = points;
        const r = this.resources;
        if (!r) return;
        this.count = Math.min(this.capacity, points.length / POINT_WORDS);
        if (this.count) r.device.queue.writeBuffer(r.points, 0, points, 0, this.count * POINT_WORDS);
        r.device.queue.writeBuffer(r.pool, 0, new Uint32Array([this.count, 0, 0, 0]));
    }

    encode(pass: GPUComputePassEncoder, ground: GPUBindGroup): void {
        const r = this.resources;
        if (!r || !this.count) return;
        pass.setPipeline(r.pipeline);
        pass.setBindGroup(0, r.group);
        pass.setBindGroup(1, r.surface);
        pass.setBindGroup(2, ground);
        pass.setBindGroup(3, r.features);
        pass.dispatchWorkgroups(Math.ceil(this.count / 64));
    }
}
