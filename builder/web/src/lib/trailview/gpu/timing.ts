import type { Gpu } from './device';

/** Readbacks in flight at most; a frame that finds none free is not timed. */
const READBACKS = 3;

/**
 * GPU time per pass from timestamp queries, when the device has them. Chrome rounds
 * timestamps to 100 µs unless it runs with `--enable-webgpu-developer-features`.
 */
export class PassTimes {
    /** Milliseconds of each pass in the last frame that was read; a pass that this frame did not run has none. */
    readonly ms: Record<string, number> = {};
    /** What the caller told `resolve()` of the frame that `ms` holds: the readback ends frames after the submit. */
    tag?: Record<string, number>;
    private readonly set?: GPUQuerySet;
    private readonly resolved?: GPUBuffer;
    private readonly free: GPUBuffer[] = [];
    private readonly written = new Set<string>();

    constructor(gpu: Gpu, private readonly passes: readonly string[]) {
        if (!gpu.device.features.has('timestamp-query')) return;
        const bytes = 16 * passes.length;
        this.set = gpu.device.createQuerySet({ type: 'timestamp', count: 2 * passes.length });
        this.resolved = gpu.buffer('timing', bytes, GPUBufferUsage.QUERY_RESOLVE | GPUBufferUsage.COPY_SRC);
        for (let i = 0; i < READBACKS; i++) this.free.push(gpu.buffer('timing', bytes, GPUBufferUsage.MAP_READ | GPUBufferUsage.COPY_DST));
    }

    writes(pass: string): GPURenderPassTimestampWrites | undefined {
        const index = this.passes.indexOf(pass);
        if (!this.set || !this.free.length || index < 0) return undefined;
        this.written.add(pass);
        return { querySet: this.set, beginningOfPassWriteIndex: 2 * index, endOfPassWriteIndex: 2 * index + 1 };
    }

    /** Encodes the readback of this frame's passes, with a tag of the frame. Call the returned function after the submit. */
    resolve(encoder: GPUCommandEncoder, tag?: Record<string, number>): () => void {
        const target = this.free.pop(), passes = [...this.written];
        this.written.clear();
        if (!this.set || !this.resolved || !target) return () => {};
        encoder.resolveQuerySet(this.set, 0, 2 * this.passes.length, this.resolved, 0);
        encoder.copyBufferToBuffer(this.resolved, 0, target, 0, target.size);
        return () => target.mapAsync(GPUMapMode.READ).then(() => {
            const times = new BigInt64Array(target.getMappedRange());
            for (const pass of this.passes) delete this.ms[pass];
            for (const pass of passes) {
                const index = this.passes.indexOf(pass);
                this.ms[pass] = Number(times[2 * index + 1] - times[2 * index]) / 1e6;
            }
            this.tag = tag;
            target.unmap();
            this.free.push(target);
        }, () => {});
    }
}
