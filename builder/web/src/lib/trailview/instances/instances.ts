import type { Gpu } from '../gpu/device';
import { wgsl } from '../gpu/wgsl';

/** Draws per model: up to three mesh tiers and the impostor tier, as TIERS in emit.wgsl. */
export const TIERS = 4;
/** Models at most, so models × TIERS draws fit the tally of emit.wgsl. */
export const MODELS = 128;
const DRAWS = MODELS * TIERS;
/** Instances that a frame stages at most; past them it drops instances and lowers the detail. */
const CAPACITY = 2 ** 17;
/** Bytes of `Instance`, `Model`, `Tally` and `Mesh` in the WGSL, and of a vertex. */
const INSTANCE_BYTES = 32, MODEL_BYTES = 32, TALLY_BYTES = 16 + 4 * DRAWS, MESH_BYTES = 16, VERTEX_BYTES = 16;
/** Lanes of the sort, as LANES in sort.wgsl. */
const SORT_LANES = 16384;
/** The stride of a dynamic uniform offset: the least that WebGPU allows. */
const STRIDE = 256;
/** The mesh pools: vertices and 32-bit indices. */
const VERTEX_POOL = 4 * 2 ** 20, INDEX_POOL = 4 * 2 ** 20;
/** The render height in pixels that the tier distances are for. */
const REFERENCE_PIXELS = 1120;
/** The share of the capacity that the detail keeps to. */
const ROOM = [0.7, 0.9];
/** Impostor layers in the atlas, and the atlas's texels a side: 8 × 8 frames of 32 texels, as FRAMES in hemi.wgsl. */
const SLOTS = 16, ATLAS = 256, FRAMES = 64;
/** Texels a side of level 0 of the depth pyramid, its levels, and the bytes of `Occlusion` in emit.wgsl. */
const PYRAMID = 512, PYRAMID_LEVELS = 10, OCCLUSION_BYTES = 96;
/** The quad of an impostor: corners at ±1. */
const QUAD = { position: [-1, -1, 0, 1, -1, 0, -1, 1, 0, 1, 1, 0], normal: [0, 0, 1, 0, 0, 1, 0, 0, 1, 0, 0, 1], color: new Array(12).fill(1), index: [0, 1, 2, 2, 1, 3] };

/** A mesh: per vertex a position, a normal and a linear colour, in a model 1 tall; three indices per triangle. */
export interface Mesh { position: ArrayLike<number>; normal: ArrayLike<number>; color: ArrayLike<number>; index: ArrayLike<number> }

/**
 * A model: its mesh per tier, finest first, and the distance in metres where each tier ends; then,
 * optionally, an impostor baked from mesh tier `from` until its own distance.
 */
export interface ModelSpec {
    tiers: { mesh: Mesh; until: number }[];
    impostor?: { from: number; until: number };
    /** The share of a tier's end distance over which it fades into the next tier. */
    fade?: number;
}

/** The last frame, for occlusion culling: its depth, its render pixels, its view projection and the eye's move since then. */
export interface LastFrame { depth: GPUTexture; pixels: [number, number]; viewProj: Float32Array; moved: [number, number, number] }

/** A compute pass that emits instances (emit.wgsl), with the ground pages in group 2. */
export interface Producer { encode(pass: GPUComputePassEncoder, ground: GPUBindGroup): void }

export interface InstanceStats { staged: number; dropped: number; lod: number; perTier: number[]; impostors: number }

interface Resources {
    gpu: Gpu;
    models: GPUBuffer;
    meshes: GPUBuffer;
    staged: GPUBuffer;
    tally: GPUBuffer;
    params: GPUBuffer;
    args: GPUBuffer;
    bases: GPUBuffer;
    lists: GPUBuffer;
    drawOf: GPUBuffer;
    vertices: GPUBuffer;
    indices: GPUBuffer;
    slots: GPUBuffer;
    bakes: GPUBuffer;
    albedo: GPUTexture;
    normal: GPUTexture;
    depth: GPUTexture;
    readbacks: GPUBuffer[];
    countDraws: GPUComputePipeline;
    placeDraws: GPUComputePipeline;
    scatter: GPUComputePipeline;
    mesh: GPURenderPipeline;
    impostor: GPURenderPipeline;
    bake: GPURenderPipeline;
    sortGroup: GPUBindGroup;
    renderGroup: GPUBindGroup;
    emptyGroup: GPUBindGroup;
    atlasGroup: GPUBindGroup;
    bakeGroup: GPUBindGroup;
    pyramid: GPUTexture;
    occlusion: GPUBuffer;
    viewport: GPUBuffer;
    hizFirst: GPUComputePipeline;
    hizDown: GPUComputePipeline;
    firstLayout: GPUBindGroupLayout;
    downGroups: GPUBindGroup[];
    /** The bind group of `first` and the depth that it reads. */
    first?: { depth: GPUTexture; group: GPUBindGroup };
}

/** Where a model's meshes lie in the pools, per tier: first index, index count, base vertex. */
type Placed = [number, number, number][];

/**
 * Instances on the drawn surface, GPU driven. Producers are compute passes that include emit.wgsl:
 * each finds its instances and their drawn heights, and `emit` culls them against the view and
 * picks the tier. The core then sorts the staged instances into one list per draw (a model's tier)
 * and draws each list with one indirect draw. A model's impostor tier draws from an atlas layer that
 * the core bakes when a frame first draws the tier. The pools have a fixed size: a frame that stages
 * more than they hold lowers the tier distances of the next frames, and the atlas keeps the models
 * that frames drew in their impostor tier most lately.
 */
export class Instances {
    private resources?: Resources;
    private specs: ModelSpec[] = [];
    private placed: Placed[] = [];
    private uploaded = false;
    /** The tier distances' factor of the detail: below 1 while the staged instances fill the pool. */
    detail = 1;
    /** The factor of the tier distances in the last frame. */
    lod = 1;
    private frame = 0;
    private stats_: InstanceStats = { staged: 0, dropped: 0, lod: 1, perTier: [0, 0, 0, 0], impostors: 0 };
    /** Per model with an atlas layer: the layer and the frame that last drew the impostor tier. */
    private readonly atlas = new Map<number, { slot: number; used: number }>();
    /** Models whose layer needs a bake. */
    private readonly unbaked = new Set<number>();
    private readonly producers: Producer[] = [];
    /** Whether the last frame baked an impostor: what the frames draw changed. */
    newContent = false;

    get stats(): InstanceStats {
        return this.stats_;
    }

    /** Whether no bake waits. */
    get idle(): boolean {
        return this.unbaked.size === 0;
    }

    /** Layout entries 0 to 6 of a producer's group 0, for its compute stage. */
    static emitEntries(): GPUBindGroupLayoutEntry[] {
        const C = GPUShaderStage.COMPUTE;
        return [
            { binding: 0, visibility: C, buffer: { type: 'uniform' } }, { binding: 1, visibility: C, buffer: { type: 'uniform' } },
            { binding: 2, visibility: C, buffer: { type: 'storage' } }, { binding: 3, visibility: C, buffer: { type: 'storage' } },
            { binding: 4, visibility: C, buffer: { type: 'uniform' } }, { binding: 5, visibility: C, buffer: { type: 'uniform' } },
            { binding: 6, visibility: C, texture: { sampleType: 'unfilterable-float' } },
        ];
    }

    /** The resources of entries 0 to 6 of a producer's group 0, after `attach`. */
    emitResources(frame: GPUBuffer): GPUBindGroupEntry[] {
        const r = this.resources!;
        return [
            ...[frame, r.models, r.staged, r.tally, r.params, r.occlusion].map((buffer, binding) => ({ binding, resource: { buffer } })),
            { binding: 6, resource: r.pyramid.createView() },
        ];
    }

    /** Adds a model and returns its index. */
    add(spec: ModelSpec): number {
        if (this.specs.length === MODELS) throw new Error(`The instances have room for ${MODELS} models.`);
        this.specs.push(spec);
        this.uploaded = false;
        return this.specs.length - 1;
    }

    /** Adds a producer, which `run` encodes each frame. */
    produce(producer: Producer): void {
        this.producers.push(producer);
    }

    /** Changes a model's meshes, such as for another season. */
    replace(model: number, spec: ModelSpec): void {
        this.specs[model] = spec;
        this.uploaded = false;
    }

    async attach(gpu: Gpu, frame: GPUBuffer, format: GPUTextureFormat, depthFormat: GPUTextureFormat, groundLayout: GPUBindGroupLayout): Promise<void> {
        this.resources = undefined;
        this.uploaded = false;
        this.atlas.clear();
        this.unbaked.clear();
        const { device } = gpu, C = GPUShaderStage.COMPUTE, V = GPUShaderStage.VERTEX, F = GPUShaderStage.FRAGMENT;
        const storage = GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST, uniform = GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST;
        const atlas = () => gpu.texture('instances', {
            size: [ATLAS, ATLAS, SLOTS], format: 'rgba8unorm', usage: GPUTextureUsage.RENDER_ATTACHMENT | GPUTextureUsage.TEXTURE_BINDING,
        }, ATLAS * ATLAS * SLOTS * 4);
        const b = {
            models: gpu.buffer('instances', MODELS * MODEL_BYTES, uniform),
            meshes: gpu.buffer('instances', DRAWS * MESH_BYTES, storage),
            staged: gpu.buffer('instances', CAPACITY * INSTANCE_BYTES, GPUBufferUsage.STORAGE),
            tally: gpu.buffer('instances', TALLY_BYTES, storage | GPUBufferUsage.COPY_SRC),
            params: gpu.buffer('instances', 16, uniform),
            args: gpu.buffer('instances', DRAWS * 20, GPUBufferUsage.STORAGE | GPUBufferUsage.INDIRECT),
            bases: gpu.buffer('instances', 2 * DRAWS * 4, GPUBufferUsage.STORAGE),
            lists: gpu.buffer('instances', CAPACITY * INSTANCE_BYTES, GPUBufferUsage.STORAGE),
            drawOf: gpu.buffer('instances', DRAWS * STRIDE, uniform),
            vertices: gpu.buffer('instances', VERTEX_POOL, GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST),
            indices: gpu.buffer('instances', INDEX_POOL, GPUBufferUsage.INDEX | GPUBufferUsage.COPY_DST),
            slots: gpu.buffer('instances', MODELS * 4, uniform),
            bakes: gpu.buffer('instances', SLOTS * STRIDE, uniform),
            albedo: atlas(),
            normal: atlas(),
            depth: gpu.texture('instances', { size: [ATLAS, ATLAS], format: 'depth32float', usage: GPUTextureUsage.RENDER_ATTACHMENT }, ATLAS * ATLAS * 4),
            readbacks: [0, 1].map(() => gpu.buffer('instances', TALLY_BYTES, GPUBufferUsage.MAP_READ | GPUBufferUsage.COPY_DST)),
        };
        const drawOf = new Uint32Array(DRAWS * STRIDE / 4);
        for (let d = 0; d < DRAWS; d++) drawOf[d * STRIDE / 4] = d;
        device.queue.writeBuffer(b.drawOf, 0, drawOf);
        const module = (label: string, path: string) => device.createShaderModule({ label, code: wgsl(path) });
        const entry = (binding: number, visibility: number, type: GPUBufferBindingType, dynamic = false): GPUBindGroupLayoutEntry =>
            ({ binding, visibility, buffer: { type, hasDynamicOffset: dynamic, minBindingSize: dynamic ? 16 : undefined } });
        const sortLayout = device.createBindGroupLayout({ entries: [
            entry(0, C, 'storage'), entry(1, C, 'read-only-storage'), entry(2, C, 'storage'), entry(3, C, 'storage'), entry(4, C, 'read-only-storage'), entry(5, C, 'storage'),
        ] });
        const renderLayout = device.createBindGroupLayout({ entries: [
            entry(0, V | F, 'uniform'), entry(1, V, 'read-only-storage'), entry(2, V, 'read-only-storage'), entry(3, V, 'uniform'), entry(4, V, 'uniform', true),
        ] });
        const atlasLayout = device.createBindGroupLayout({ entries: [
            { binding: 0, visibility: F, texture: { viewDimension: '2d-array' } }, { binding: 1, visibility: F, texture: { viewDimension: '2d-array' } },
            { binding: 2, visibility: F, sampler: {} }, entry(3, V, 'uniform'),
        ] });
        const bakeLayout = device.createBindGroupLayout({ entries: [entry(0, V, 'uniform', true)] });
        const firstLayout = device.createBindGroupLayout({ entries: [
            { binding: 0, visibility: C, texture: { sampleType: 'depth' } }, { binding: 1, visibility: C, storageTexture: { format: 'r32float' } }, entry(4, C, 'uniform'),
        ] });
        const downLayout = device.createBindGroupLayout({ entries: [
            { binding: 2, visibility: C, texture: { sampleType: 'unfilterable-float' } }, { binding: 3, visibility: C, storageTexture: { format: 'r32float' } },
        ] });
        const hizModule = module('depth pyramid', 'instances/hiz.wgsl');
        const [hizFirst, hizDown] = await Promise.all([
            device.createComputePipelineAsync({ label: 'pyramid first', layout: device.createPipelineLayout({ bindGroupLayouts: [firstLayout] }), compute: { module: hizModule, entryPoint: 'first' } }),
            device.createComputePipelineAsync({ label: 'pyramid down', layout: device.createPipelineLayout({ bindGroupLayouts: [downLayout] }), compute: { module: hizModule, entryPoint: 'down' } }),
        ]);
        const pyramid = gpu.texture('instances', {
            size: [PYRAMID, PYRAMID], mipLevelCount: PYRAMID_LEVELS, format: 'r32float', usage: GPUTextureUsage.STORAGE_BINDING | GPUTextureUsage.TEXTURE_BINDING,
        }, PYRAMID * PYRAMID * 4 * 4 / 3);
        const level = (k: number) => pyramid.createView({ baseMipLevel: k, mipLevelCount: 1 });
        const downGroups = Array.from({ length: PYRAMID_LEVELS - 1 }, (_, k) => device.createBindGroup({
            layout: downLayout, entries: [{ binding: 2, resource: level(k) }, { binding: 3, resource: level(k + 1) }],
        }));
        const emptyLayout = device.createBindGroupLayout({ entries: [] });
        const sortModule = module('instances sort', 'instances/sort.wgsl'), drawModule = module('instances draw', 'instances/draw.wgsl');
        const impostorModule = module('impostor', 'instances/impostor.wgsl'), bakeModule = module('impostor bake', 'instances/impostor-bake.wgsl');
        const vertexLayout: GPUVertexBufferLayout = {
            arrayStride: VERTEX_BYTES, attributes: [
                { shaderLocation: 0, offset: 0, format: 'snorm16x4' }, { shaderLocation: 1, offset: 8, format: 'snorm8x4' }, { shaderLocation: 2, offset: 12, format: 'unorm8x4' },
            ],
        };
        const depthStencil: GPUDepthStencilState = { format: depthFormat, depthWriteEnabled: true, depthCompare: 'greater' };
        const sortPipeline = device.createPipelineLayout({ bindGroupLayouts: [sortLayout] });
        const [countDraws, placeDraws, scatter, mesh, impostor, bake] = await Promise.all([
            device.createComputePipelineAsync({ label: 'instances count', layout: sortPipeline, compute: { module: sortModule, entryPoint: 'count_draws' } }),
            device.createComputePipelineAsync({ label: 'instances place', layout: sortPipeline, compute: { module: sortModule, entryPoint: 'place_draws' } }),
            device.createComputePipelineAsync({ label: 'instances scatter', layout: sortPipeline, compute: { module: sortModule, entryPoint: 'scatter' } }),
            device.createRenderPipelineAsync({
                label: 'instances mesh', layout: device.createPipelineLayout({ bindGroupLayouts: [renderLayout, emptyLayout, groundLayout] }),
                vertex: { module: drawModule, entryPoint: 'mesh_vertex', buffers: [vertexLayout] },
                fragment: { module: drawModule, entryPoint: 'mesh_fragment', targets: [{ format }] },
                // Leaves and twigs are single faces, seen from both sides.
                primitive: { topology: 'triangle-list', cullMode: 'none' }, depthStencil,
            }),
            device.createRenderPipelineAsync({
                label: 'impostors', layout: device.createPipelineLayout({ bindGroupLayouts: [renderLayout, atlasLayout, groundLayout] }),
                vertex: { module: impostorModule, entryPoint: 'impostor_vertex', buffers: [vertexLayout] },
                fragment: { module: impostorModule, entryPoint: 'impostor_fragment', targets: [{ format }] },
                primitive: { topology: 'triangle-list', cullMode: 'none' }, depthStencil,
            }),
            device.createRenderPipelineAsync({
                label: 'impostor bake', layout: device.createPipelineLayout({ bindGroupLayouts: [renderLayout, bakeLayout] }),
                vertex: { module: bakeModule, entryPoint: 'bake_vertex', buffers: [vertexLayout] },
                fragment: { module: bakeModule, entryPoint: 'bake_fragment', targets: [{ format: 'rgba8unorm' }, { format: 'rgba8unorm' }] },
                primitive: { topology: 'triangle-list', cullMode: 'none' },
                depthStencil: { format: 'depth32float', depthWriteEnabled: true, depthCompare: 'greater' },
            }),
        ]);
        const group = (layout: GPUBindGroupLayout, resources: (GPUBuffer | [GPUBuffer, number] | GPUTextureView | GPUSampler)[]) => device.createBindGroup({
            layout, entries: resources.map((resource, binding) => ({
                binding, resource: resource instanceof GPUBuffer ? { buffer: resource } : Array.isArray(resource) ? { buffer: resource[0], size: resource[1] } : resource,
            })),
        });
        const layers = (texture: GPUTexture) => texture.createView({ dimension: '2d-array' });
        this.resources = {
            gpu, ...b, countDraws, placeDraws, scatter, mesh, impostor, bake, pyramid, hizFirst, hizDown, firstLayout, downGroups,
            occlusion: gpu.buffer('instances', OCCLUSION_BYTES, uniform), viewport: gpu.buffer('instances', 16, uniform),
            sortGroup: group(sortLayout, [b.tally, b.meshes, b.args, b.bases, b.staged, b.lists]),
            renderGroup: group(renderLayout, [frame, b.lists, b.bases, b.models, [b.drawOf, 16]]),
            emptyGroup: group(emptyLayout, []),
            atlasGroup: group(atlasLayout, [layers(b.albedo), layers(b.normal), device.createSampler({ magFilter: 'linear', minFilter: 'linear' }), b.slots]),
            bakeGroup: group(bakeLayout, [[b.bakes, 16]]),
        };
    }

    /** Starts a frame: the models when they changed, the detail for the render's height in pixels, and an empty tally. */
    begin(pixels: number): void {
        const r = this.resources;
        if (!r) return;
        this.frame++;
        if (!this.uploaded) this.upload(r);
        this.lod = this.detail * pixels / REFERENCE_PIXELS;
        const params = new ArrayBuffer(16);
        new Float32Array(params).set([this.lod, 0, this.detail]);
        new Uint32Array(params)[1] = CAPACITY;
        r.gpu.device.queue.writeBuffer(r.params, 0, params);
        r.gpu.device.queue.writeBuffer(r.tally, 0, new Uint32Array(TALLY_BYTES / 4));
    }

    /**
     * Builds the depth pyramid of the last frame, which `hidden` of emit.wgsl reads; without a last
     * frame, nothing is hidden.
     */
    occlude(encoder: GPUCommandEncoder, last: LastFrame | undefined): void {
        const r = this.resources;
        if (!r) return;
        const data = new ArrayBuffer(OCCLUSION_BYTES), f = new Float32Array(data);
        if (!last) return r.gpu.device.queue.writeBuffer(r.occlusion, 0, data);
        const block = Math.max(4, Math.ceil(Math.max(...last.pixels) / PYRAMID));
        f.set(last.viewProj);
        f.set([...last.moved, 1, ...last.pixels, block], 16);
        r.gpu.device.queue.writeBuffer(r.occlusion, 0, data);
        r.gpu.device.queue.writeBuffer(r.viewport, 0, new Uint32Array([...last.pixels, block, 0]));
        if (r.first?.depth !== last.depth) r.first = {
            depth: last.depth, group: r.gpu.device.createBindGroup({ layout: r.firstLayout, entries: [
                { binding: 0, resource: last.depth.createView() }, { binding: 1, resource: r.pyramid.createView({ baseMipLevel: 0, mipLevelCount: 1 }) }, { binding: 4, resource: { buffer: r.viewport } },
            ] }),
        };
        const pass = encoder.beginComputePass({ label: 'depth pyramid' });
        pass.setPipeline(r.hizFirst);
        pass.setBindGroup(0, r.first.group);
        pass.dispatchWorkgroups(Math.ceil(last.pixels[0] / block / 8), Math.ceil(last.pixels[1] / block / 8));
        pass.setPipeline(r.hizDown);
        r.downGroups.forEach((group, k) => {
            pass.setBindGroup(0, group);
            pass.dispatchWorkgroups(Math.ceil((PYRAMID >> (k + 1)) / 8), Math.ceil((PYRAMID >> (k + 1)) / 8));
        });
        pass.end();
    }

    /** Bakes the impostors that the frames asked for, before the frame draws them. */
    bake(encoder: GPUCommandEncoder): void {
        const r = this.resources;
        this.newContent = false;
        if (!r || !this.uploaded || !this.unbaked.size) return;
        for (const model of this.unbaked) {
            const slot = this.atlas.get(model)?.slot, spec = this.specs[model];
            if (slot === undefined || !spec?.impostor) continue;
            const [first, count, base] = this.placed[model][spec.impostor.from];
            const data = new ArrayBuffer(16);
            new Uint32Array(data)[0] = model;
            new Float32Array(data)[1] = radius(spec);
            r.gpu.device.queue.writeBuffer(r.bakes, slot * STRIDE, data);
            const layer = (texture: GPUTexture) => texture.createView({ dimension: '2d', baseArrayLayer: slot, arrayLayerCount: 1 });
            const pass = encoder.beginRenderPass({
                label: 'impostor bake',
                colorAttachments: [r.albedo, r.normal].map(texture => ({ view: layer(texture), loadOp: 'clear' as const, storeOp: 'store' as const, clearValue: [0, 0, 0, 0] })),
                depthStencilAttachment: { view: r.depth.createView(), depthClearValue: 0, depthLoadOp: 'clear', depthStoreOp: 'discard' },
            });
            pass.setPipeline(r.bake);
            pass.setBindGroup(0, r.renderGroup, [0]);
            pass.setBindGroup(1, r.bakeGroup, [slot * STRIDE]);
            pass.setVertexBuffer(0, r.vertices);
            pass.setIndexBuffer(r.indices, 'uint32');
            pass.drawIndexed(count, FRAMES, first, base);
            pass.end();
        }
        this.unbaked.clear();
        this.newContent = true;
    }

    /** Runs the producers, then sorts what they staged into the draw lists. */
    run(pass: GPUComputePassEncoder, ground: GPUBindGroup): void {
        const r = this.resources;
        if (!r) return;
        for (const producer of this.producers) producer.encode(pass, ground);
        // A fixed number of lanes, each looping over the staged instances: the CPU does not know their count.
        pass.setBindGroup(0, r.sortGroup);
        pass.setPipeline(r.countDraws);
        pass.dispatchWorkgroups(SORT_LANES / 64);
        pass.setPipeline(r.placeDraws);
        pass.dispatchWorkgroups(1);
        pass.setPipeline(r.scatter);
        pass.dispatchWorkgroups(SORT_LANES / 64);
    }

    /** Encodes the readback of the tally, for the stats, the detail and the atlas. Call the returned function after the submit. */
    read(encoder: GPUCommandEncoder): () => void {
        const r = this.resources, readback = r?.readbacks.pop();
        if (!r || !readback) return () => {};
        encoder.copyBufferToBuffer(r.tally, 0, readback, 0, TALLY_BYTES);
        const frame = this.frame;
        return () => readback.mapAsync(GPUMapMode.READ).then(() => {
            const tally = new Uint32Array(readback.getMappedRange());
            const perTier = [0, 1, 2, 3].map(t => this.specs.reduce((sum, _, m) => sum + tally[4 + m * TIERS + t], 0));
            this.stats_ = { staged: tally[0], dropped: tally[1], lod: this.lod, perTier, impostors: this.atlas.size };
            // A full pool draws its tiers nearer, until it has room again.
            const used = tally[0] / CAPACITY;
            this.detail = used > ROOM[1] ? this.detail * 0.9 : used < ROOM[0] ? Math.min(1, this.detail / 0.97) : this.detail;
            this.specs.forEach((spec, m) => {
                if (spec.impostor && tally[4 + m * TIERS + spec.tiers.length] > 0) this.wantImpostor(r, m, frame);
            });
            readback.unmap();
            r.readbacks.push(readback);
        }, () => {});
    }

    draw(pass: GPURenderPassEncoder, ground: GPUBindGroup): void {
        const r = this.resources;
        if (!r || !this.uploaded) return;
        pass.setPipeline(r.mesh);
        pass.setBindGroup(1, r.emptyGroup);
        pass.setBindGroup(2, ground);
        pass.setVertexBuffer(0, r.vertices);
        pass.setIndexBuffer(r.indices, 'uint32');
        const indirect = (d: number) => {
            pass.setBindGroup(0, r.renderGroup, [d * STRIDE]);
            pass.drawIndexedIndirect(r.args, d * 20);
        };
        this.specs.forEach((spec, m) => spec.tiers.forEach((_, t) => indirect(m * TIERS + t)));
        if (!this.atlas.size) return;
        pass.setPipeline(r.impostor);
        pass.setBindGroup(1, r.atlasGroup);
        for (const [m] of this.atlas) if (!this.unbaked.has(m)) indirect(m * TIERS + this.specs[m].tiers.length);
    }

    /** Gives a model a layer of the atlas, the layer that the frames drew least lately when none is free, and bakes it. */
    private wantImpostor(r: Resources, model: number, frame: number): void {
        const held = this.atlas.get(model);
        if (held) return void (held.used = frame);
        let slot = this.atlas.size < SLOTS ? this.atlas.size : -1;
        if (slot < 0) {
            const [oldest, entry] = [...this.atlas].reduce((a, b) => (b[1].used < a[1].used ? b : a));
            if (entry.used >= frame) return;
            this.atlas.delete(oldest);
            slot = entry.slot;
        }
        this.atlas.set(model, { slot, used: frame });
        this.unbaked.add(model);
        const slots = new Uint32Array(MODELS);
        for (const [m, held] of this.atlas) slots[m] = held.slot;
        r.gpu.device.queue.writeBuffer(r.slots, 0, slots);
    }

    /** Writes the models and their meshes into the pools; every impostor bakes again. */
    private upload(r: Resources): void {
        const models = new ArrayBuffer(MODELS * MODEL_BYTES), f = new Float32Array(models);
        const meshes = new Int32Array(DRAWS * MESH_BYTES / 4);
        let vertex = 0, index = 0;
        const put = (mesh: Mesh, extent: number): [number, number, number] => {
            const count = mesh.position.length / 3;
            if ((vertex + count) * VERTEX_BYTES > VERTEX_POOL || (index + mesh.index.length) * 4 > INDEX_POOL) throw new Error('The instance meshes do not fit their pools.');
            r.gpu.device.queue.writeBuffer(r.vertices, vertex * VERTEX_BYTES, quantize(mesh, extent));
            r.gpu.device.queue.writeBuffer(r.indices, index * 4, Uint32Array.from(mesh.index));
            const at: [number, number, number] = [index, mesh.index.length, vertex];
            vertex += count;
            index += mesh.index.length;
            return at;
        };
        const quad = put(QUAD, 1);
        this.placed = this.specs.map((spec, m) => {
            const { tiers } = spec;
            if (!tiers.length) return [];
            const extent = Math.max(...tiers.map(({ mesh }) => largest(mesh.position)));
            const placed = tiers.map(({ mesh, until }, t) => {
                const at = put(mesh, extent);
                meshes.set([at[1], at[0], at[2], 0], (m * TIERS + t) * 4);
                f[m * 8 + t] = until;
                return at;
            });
            if (spec.impostor) {
                meshes.set([quad[1], quad[0], quad[2], 0], (m * TIERS + tiers.length) * 4);
                f[m * 8 + tiers.length] = spec.impostor.until;
                if (this.atlas.has(m)) this.unbaked.add(m);
            }
            f.set([extent, radius(spec), spec.fade ?? 0.15, spread(spec)], m * 8 + 4);
            return placed;
        });
        r.gpu.device.queue.writeBuffer(r.models, 0, models);
        r.gpu.device.queue.writeBuffer(r.meshes, 0, meshes);
        this.uploaded = true;
    }
}

/** The largest absolute value. */
function largest(values: ArrayLike<number>): number {
    let most = 0;
    for (let i = 0; i < values.length; i++) most = Math.max(most, Math.abs(values[i]));
    return most;
}

/** The radius of a sphere about (0, 0, 0.5) that holds every mesh of a model 1 tall. */
function radius(spec: ModelSpec): number {
    let most = 0;
    for (const { mesh: { position: p } } of spec.tiers) for (let i = 0; i < p.length; i += 3) most = Math.max(most, Math.hypot(p[i], p[i + 1], p[i + 2] - 0.5));
    return most;
}

/** The farthest that a vertex of a model lies from its axis. */
function spread(spec: ModelSpec): number {
    let most = 0;
    for (const { mesh: { position: p } } of spec.tiers) for (let i = 0; i < p.length; i += 3) most = Math.max(most, Math.hypot(p[i], p[i + 1]));
    return most;
}

/**
 * The vertices of a mesh as the instance pipeline reads them, 16 bytes each: the position over
 * `extent` as snorm16 × 4, the normal as snorm8 × 4 and the colour in sRGB as unorm8 × 4.
 */
export function quantize(mesh: Mesh, extent: number): ArrayBuffer {
    const count = mesh.position.length / 3, out = new ArrayBuffer(count * VERTEX_BYTES);
    const i16 = new Int16Array(out), i8 = new Int8Array(out), u8 = new Uint8Array(out);
    const srgb = (c: number) => Math.round(255 * Math.min(1, Math.max(0, c <= 0.0031308 ? 12.92 * c : 1.055 * c ** (1 / 2.4) - 0.055)));
    for (let k = 0; k < count; k++) {
        for (let d = 0; d < 3; d++) {
            i16[8 * k + d] = Math.round(32767 * mesh.position[3 * k + d] / extent);
            i8[16 * k + 8 + d] = Math.round(127 * (mesh.normal[3 * k + d] ?? 0));
            u8[16 * k + 12 + d] = srgb(mesh.color[3 * k + d] ?? 1);
        }
        u8[16 * k + 15] = 255;
    }
    return out;
}
