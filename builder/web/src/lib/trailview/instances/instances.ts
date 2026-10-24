import type { Gpu } from '../gpu/device';
import { wgsl } from '../gpu/wgsl';

/** Draws per model: up to three mesh tiers and the impostor tier, as TIERS in emit.wgsl. */
const TIERS = 4;
/** Models at most, as the uniform arrays of emit.wgsl, draw.wgsl and impostor.wgsl and the draws of sort.wgsl. */
const MODELS = 256;
const DRAWS = MODELS * TIERS;
/** Instances that a frame stages at most; past them it drops instances and lowers the detail. */
const CAPACITY = 2 ** 17;
/** Bytes of `Instance`, `Model`, `Tally` and `Mesh` in the WGSL, and of a vertex. */
const INSTANCE_BYTES = 32, MODEL_BYTES = 80, TALLY_BYTES = 16 + 4 * DRAWS, MESH_BYTES = 16, VERTEX_BYTES = 16;
/** The phases of a tree's year, as PHASES of phenology.wgsl, and the bytes of a colour matrix (`mat3x3f`). */
const PHASES = 4, TONE_BYTES = 48;
/** Lanes of the sort, as LANES in sort.wgsl. */
const SORT_LANES = 16384;
/** The stride of a dynamic uniform offset: the least that WebGPU allows. */
const STRIDE = 256;
/** The mesh pools: vertices and 32-bit indices. */
const VERTEX_POOL = 6 * 2 ** 20, INDEX_POOL = 6 * 2 ** 20;
/**
 * The mesh tiers that draw without the dither's discard, as WHOLE_TIERS of emit.wgsl: an instance that
 * draws on a share of its pixels stages in a farther tier.
 */
const WHOLE_TIERS = 2;
/** A tree's sink table (`sinks`): its slopes, SINK_STEP apart from SINK_STEP, as in emit.wgsl. */
const SINK_STEP = 0.25, SINK_SLOPES = 8;
/** The render height in pixels that the tier distances are for. */
const REFERENCE_PIXELS = 1120;
/** The share of the capacity that the detail keeps to. */
const ROOM = [0.7, 0.9];
/**
 * Impostor layers in the atlas, more than the tree species of a region (SPECIES of forest.ts); the
 * atlas's texels a side, 8 × 8 frames (FRAMES of hemi.wgsl) of 32 texels; its mip levels, as
 * ATLAS_LEVELS of impostor.wgsl; the frames of a bake. A layer leaves only after this many frames unused.
 */
const SLOTS = 32, ATLAS = 256, ATLAS_LEVELS = 6, FRAMES = 64, UNUSED_FRAMES = 60;
/** Texels a side of level 0 of the depth pyramid, its levels, and the bytes of `Occlusion` in emit.wgsl. */
const PYRAMID = 512, PYRAMID_LEVELS = 10, OCCLUSION_BYTES = 96;
/** The quad of an impostor: corners at ±1, and a middle row at y 0 that impostor.wgsl places at the crown's base. */
const QUAD = { position: [-1, -1, 0, 1, -1, 0, -1, 0, 0, 1, 0, 0, -1, 1, 0, 1, 1, 0], normal: new Array(6).fill([0, 0, 1]).flat(), color: new Array(18).fill(1), index: [0, 1, 2, 2, 1, 3, 2, 3, 4, 4, 3, 5] };

/**
 * A mesh: per vertex a position, a normal and a linear colour, in a model 1 tall; three indices per
 * triangle. A tree's first `trunk` vertices are its trunk, which stretches to meet its crown when the
 * crown lifts (draw.wgsl).
 */
export interface Mesh { position: ArrayLike<number>; normal: ArrayLike<number>; color: ArrayLike<number>; index: ArrayLike<number>; trunk?: number }

/**
 * A model: its mesh per tier, finest first, and the distance in metres where each tier ends; then,
 * optionally, an impostor baked from mesh tier `from` until its own distance.
 */
export interface ModelSpec {
    tiers: { mesh: Mesh; until: number }[];
    impostor?: { from: number; until: number };
    /** The share of a tier's end distance over which its instances switch to the next tier. */
    fade?: number;
    /** A tree's crown: its lowest point in model units (draw.wgsl). */
    crown?: number;
    /** How far it takes its hue from the ground under it, 0 to 1 (draw.wgsl). */
    tie?: number;
    /** The height in metres of an instance whose tiers end at `until`; the tiers of others end in proportion. */
    sized?: number;
    /**
     * A tree's year (vegetation/phenology.ts): per phase, winter first, the model whose mesh tiers its
     * instances draw in that phase, the model whose impostor they draw (`model` by default), and the
     * colour matrix (3 × 3, columns first) of that impostor's model's crown, none for its own colours.
     * Without phases a model draws itself all year.
     */
    phases?: { model: number; impostor?: number; tone?: number[] }[];
}

/** The last frame, for occlusion culling: its depth, its render pixels, its view projection and the eye's move since then. */
export interface LastFrame { depth: GPUTexture; pixels: [number, number]; viewProj: Float32Array; moved: [number, number, number] }

/** A bind group and its layout. */
export interface Bound { layout: GPUBindGroupLayout; group: GPUBindGroup }

/** What a producer binds besides group 0: the terrain's surface (group 1), the layout of the ground pages (group 2) and the feature mask (group 3). */
export interface Surfaces { surface: Bound; features: Bound; ground: GPUBindGroupLayout }

/** A compute pass that emits instances (emit.wgsl), with the ground pages in group 2. The core attaches it on each device. */
export interface Producer {
    attach(gpu: Gpu, frame: GPUBuffer, surfaces: Surfaces): Promise<void>;
    encode(pass: GPUComputePassEncoder, ground: GPUBindGroup): void;
}

export interface InstanceStats { staged: number; dropped: number; lod: number; perTier: number[]; impostors: number }

interface Resources {
    gpu: Gpu;
    models: GPUBuffer;
    phases: GPUBuffer;
    tones: GPUBuffer;
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
    outlines: GPUBuffer;
    bakes: GPUBuffer;
    albedo: GPUTexture;
    normal: GPUTexture;
    depth: GPUTexture;
    readbacks: GPUBuffer[];
    countDraws: GPUComputePipeline;
    placeDraws: GPUComputePipeline;
    scatter: GPUComputePipeline;
    mesh: GPURenderPipeline;
    whole: GPURenderPipeline;
    impostor: GPURenderPipeline;
    bake: GPURenderPipeline;
    mips: GPUComputePipeline;
    /** Per mip level above the first, the group that reads the level below it. */
    mipGroups: GPUBindGroup[];
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
    /** The meshes of the draws, as `Mesh` of sort.wgsl, and where the impostor quad lies. */
    private meshTable = new Int32Array(0);
    private quad: [number, number, number] = [0, 0, 0];
    private uploaded = false;
    /** The tier distances' factor of the detail: below 1 while the staged instances fill the pool. */
    private detail = 1;
    /** The factor of the tier distances in the last frame. */
    private lod = 1;
    private frame = 0;
    private stats_: InstanceStats = { staged: 0, dropped: 0, lod: 1, perTier: [0, 0, 0, 0], impostors: 0 };
    /** Per model with an atlas layer: the layer and the frame that last drew the impostor tier. */
    private readonly atlas = new Map<number, { slot: number; used: number }>();
    /** Models whose layer needs a bake. */
    private readonly unbaked = new Set<number>();
    private readonly producers: Producer[] = [];
    /** The impostor outlines of each mesh and radius (`outline`). */
    private readonly outlined = new WeakMap<Mesh, { radius: number; outlines: Float32Array<ArrayBuffer> }>();
    /** What `attach` took, for a producer that comes later. */
    private attached?: [Gpu, GPUBuffer, Surfaces];
    /** Whether the last frame baked an impostor: what the frames draw changed. */
    newContent = false;
    /** The day of the year (0 is 1 January), which sets the phase of each tree. */
    day = 0;

    get stats(): InstanceStats {
        return this.stats_;
    }

    /** Whether no bake waits. */
    get idle(): boolean {
        return this.unbaked.size === 0;
    }

    /** Layout entries 0 to 7 of a producer's group 0, for its compute stage. */
    static emitEntries(): GPUBindGroupLayoutEntry[] {
        const C = GPUShaderStage.COMPUTE;
        return [
            { binding: 0, visibility: C, buffer: { type: 'uniform' } }, { binding: 1, visibility: C, buffer: { type: 'uniform' } },
            { binding: 2, visibility: C, buffer: { type: 'storage' } }, { binding: 3, visibility: C, buffer: { type: 'storage' } },
            { binding: 4, visibility: C, buffer: { type: 'uniform' } }, { binding: 5, visibility: C, buffer: { type: 'uniform' } },
            { binding: 6, visibility: C, texture: { sampleType: 'unfilterable-float' } }, { binding: 7, visibility: C, buffer: { type: 'uniform' } },
        ];
    }

    /** The resources of entries 0 to 7 of a producer's group 0, after `attach`. */
    emitResources(frame: GPUBuffer): GPUBindGroupEntry[] {
        const r = this.resources!;
        return [
            ...[frame, r.models, r.staged, r.tally, r.params, r.occlusion].map((buffer, binding) => ({ binding, resource: { buffer } })),
            { binding: 6, resource: r.pyramid.createView() }, { binding: 7, resource: { buffer: r.phases } },
        ];
    }

    /**
     * Adds models with consecutive indexes and returns the first. Where they do not fit in MODELS it
     * warns and returns undefined: they draw nothing, and the rest go on.
     */
    add(...specs: ModelSpec[]): number | undefined {
        if (this.specs.length + specs.length > MODELS) return void console.warn(`The instances have room for ${MODELS} models: ${specs.length} draw nothing.`);
        this.specs.push(...specs);
        this.uploaded = false;
        return this.specs.length - specs.length;
    }

    /** Adds a producer, which `run` encodes each frame. */
    produce(producer: Producer): void {
        this.producers.push(producer);
        if (this.attached) void attachProducer(producer, ...this.attached);
    }

    /** Changes a model's meshes, such as for another season. */
    replace(model: number, spec: ModelSpec): void {
        this.specs[model] = spec;
        this.uploaded = false;
    }

    /** Takes the pools on a device, then attaches the producers. */
    async attach(gpu: Gpu, frame: GPUBuffer, format: GPUTextureFormat, depthFormat: GPUTextureFormat, groundLayout: GPUBindGroupLayout, surfaces: Surfaces): Promise<void> {
        this.resources = undefined;
        this.attached = undefined;
        this.uploaded = false;
        this.atlas.clear();
        this.unbaked.clear();
        const { device } = gpu, C = GPUShaderStage.COMPUTE, V = GPUShaderStage.VERTEX, F = GPUShaderStage.FRAGMENT;
        const storage = GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST, uniform = GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST;
        const atlas = () => gpu.texture('instances', {
            size: [ATLAS, ATLAS, SLOTS], mipLevelCount: ATLAS_LEVELS, format: 'rgba8unorm',
            usage: GPUTextureUsage.RENDER_ATTACHMENT | GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.STORAGE_BINDING,
        }, ATLAS * ATLAS * SLOTS * 4 * 4 / 3);
        const b = {
            models: gpu.buffer('instances', MODELS * MODEL_BYTES, uniform),
            phases: gpu.buffer('instances', MODELS * 16, uniform),
            tones: gpu.buffer('instances', MODELS * PHASES * TONE_BYTES, storage),
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
            outlines: gpu.buffer('instances', MODELS * FRAMES * 16, storage),
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
            entry(5, V | F, 'read-only-storage'),
        ] });
        const atlasLayout = device.createBindGroupLayout({ entries: [
            { binding: 0, visibility: F, texture: { viewDimension: '2d-array' } }, { binding: 1, visibility: F, texture: { viewDimension: '2d-array' } },
            { binding: 2, visibility: F, sampler: {} }, entry(3, V, 'uniform'), entry(4, V, 'read-only-storage'),
        ] });
        const bakeLayout = device.createBindGroupLayout({ entries: [entry(0, V | C, 'uniform', true)] });
        const mipLayout = device.createBindGroupLayout({ entries: [
            ...[0, 1].map(binding => ({ binding, visibility: C, texture: { viewDimension: '2d-array' as const } })),
            ...[2, 3].map(binding => ({ binding, visibility: C, storageTexture: { format: 'rgba8unorm' as const, viewDimension: '2d-array' as const } })),
        ] });
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
        const mipModule = module('impostor mips', 'instances/impostor-mips.wgsl');
        const vertexLayout: GPUVertexBufferLayout = {
            arrayStride: VERTEX_BYTES, attributes: [
                { shaderLocation: 0, offset: 0, format: 'snorm16x4' }, { shaderLocation: 1, offset: 8, format: 'snorm8x4' }, { shaderLocation: 2, offset: 12, format: 'unorm8x4' },
            ],
        };
        const depthStencil: GPUDepthStencilState = { format: depthFormat, depthWriteEnabled: true, depthCompare: 'greater' };
        const sortPipeline = device.createPipelineLayout({ bindGroupLayouts: [sortLayout] });
        const meshPipeline = (fragment: string) => device.createRenderPipelineAsync({
            label: 'instances mesh', layout: device.createPipelineLayout({ bindGroupLayouts: [renderLayout, emptyLayout, groundLayout] }),
            vertex: { module: drawModule, entryPoint: 'mesh_vertex', buffers: [vertexLayout] },
            fragment: { module: drawModule, entryPoint: fragment, targets: [{ format }] },
            // Leaves and twigs are single faces, seen from both sides.
            primitive: { topology: 'triangle-list', cullMode: 'none' }, depthStencil,
        });
        const [countDraws, placeDraws, scatter, mesh, whole, impostor, bake, mips] = await Promise.all([
            device.createComputePipelineAsync({ label: 'instances count', layout: sortPipeline, compute: { module: sortModule, entryPoint: 'count_draws' } }),
            device.createComputePipelineAsync({ label: 'instances place', layout: sortPipeline, compute: { module: sortModule, entryPoint: 'place_draws' } }),
            device.createComputePipelineAsync({ label: 'instances scatter', layout: sortPipeline, compute: { module: sortModule, entryPoint: 'scatter' } }),
            meshPipeline('mesh_fragment'),
            meshPipeline('mesh_fragment_whole'),
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
            device.createComputePipelineAsync({ label: 'impostor mips', layout: device.createPipelineLayout({ bindGroupLayouts: [mipLayout, bakeLayout] }), compute: { module: mipModule, entryPoint: 'down' } }),
        ]);
        const group = (layout: GPUBindGroupLayout, resources: (GPUBuffer | [GPUBuffer, number] | GPUTextureView | GPUSampler)[]) => device.createBindGroup({
            layout, entries: resources.map((resource, binding) => ({
                binding, resource: resource instanceof GPUBuffer ? { buffer: resource } : Array.isArray(resource) ? { buffer: resource[0], size: resource[1] } : resource,
            })),
        });
        const layers = (texture: GPUTexture, level?: number) => texture.createView({ dimension: '2d-array', ...(level === undefined ? {} : { baseMipLevel: level, mipLevelCount: 1 }) });
        this.resources = {
            gpu, ...b, countDraws, placeDraws, scatter, mesh, whole, impostor, bake, mips, pyramid, hizFirst, hizDown, firstLayout, downGroups,
            occlusion: gpu.buffer('instances', OCCLUSION_BYTES, uniform), viewport: gpu.buffer('instances', 16, uniform),
            sortGroup: group(sortLayout, [b.tally, b.meshes, b.args, b.bases, b.staged, b.lists]),
            renderGroup: group(renderLayout, [frame, b.lists, b.bases, b.models, [b.drawOf, 16], b.tones]),
            emptyGroup: group(emptyLayout, []),
            atlasGroup: group(atlasLayout, [layers(b.albedo), layers(b.normal), device.createSampler({ magFilter: 'linear', minFilter: 'linear', mipmapFilter: 'linear' }), b.slots, b.outlines]),
            bakeGroup: group(bakeLayout, [[b.bakes, 16]]),
            mipGroups: Array.from({ length: ATLAS_LEVELS - 1 }, (_, k) => group(mipLayout, [layers(b.albedo, k), layers(b.normal, k), layers(b.albedo, k + 1), layers(b.normal, k + 1)])),
        };
        this.attached = [gpu, frame, surfaces];
        await Promise.all(this.producers.map(producer => attachProducer(producer, gpu, frame, surfaces)));
    }

    /** Starts a frame: the models when they changed, the detail for the render's height in pixels, and an empty tally. */
    begin(pixels: number): void {
        const r = this.resources;
        if (!r) return;
        this.frame++;
        if (!this.uploaded) this.upload(r);
        this.lod = this.detail * pixels / REFERENCE_PIXELS;
        const params = new ArrayBuffer(16);
        new Float32Array(params).set([this.lod, 0, this.detail, this.day]);
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
        if (!r || !this.uploaded) return;
        for (const model of this.unbaked) {
            const slot = this.atlas.get(model)?.slot, spec = this.specs[model], placed = this.placed[model];
            if (slot === undefined || !spec?.impostor || !placed.length) continue;
            const [first, count, base] = placed[spec.impostor.from];
            r.gpu.device.queue.writeBuffer(r.outlines, model * FRAMES * 16, this.outline(spec.tiers[spec.impostor.from].mesh, radius(spec)));
            const data = new ArrayBuffer(16);
            new Uint32Array(data).set([model, 0, slot]);
            new Float32Array(data)[1] = radius(spec);
            r.gpu.device.queue.writeBuffer(r.bakes, slot * STRIDE, data);
            const layer = (texture: GPUTexture) => texture.createView({ dimension: '2d', baseArrayLayer: slot, arrayLayerCount: 1, mipLevelCount: 1 });
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
            const mips = encoder.beginComputePass({ label: 'impostor mips' });
            mips.setPipeline(r.mips);
            mips.setBindGroup(1, r.bakeGroup, [slot * STRIDE]);
            r.mipGroups.forEach((group, k) => {
                mips.setBindGroup(0, group);
                mips.dispatchWorkgroups(Math.ceil((ATLAS >> (k + 1)) / 8), Math.ceil((ATLAS >> (k + 1)) / 8));
            });
            mips.end();
        }
        this.newContent = this.unbaked.size > 0;
        this.unbaked.clear();
        this.aim(r);
    }

    /** Points each impostor draw at its quad where the atlas holds the model's layer, else at the model's coarsest mesh. */
    private aim(r: Resources): void {
        let changed = false;
        this.specs.forEach((spec, m) => {
            const placed = this.placed[m];
            if (!spec.impostor || !placed?.length) return;
            const [first, count, base] = this.atlas.has(m) ? this.quad : placed[placed.length - 1], at = (m * TIERS + spec.tiers.length) * 4;
            if (this.meshTable[at] === count && this.meshTable[at + 1] === first) return;
            this.meshTable.set([count, first, base, 0], at);
            changed = true;
        });
        if (changed) r.gpu.device.queue.writeBuffer(r.meshes, 0, this.meshTable);
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
        pass.setBindGroup(1, r.emptyGroup);
        pass.setBindGroup(2, ground);
        pass.setVertexBuffer(0, r.vertices);
        pass.setIndexBuffer(r.indices, 'uint32');
        const indirect = (d: number) => {
            pass.setBindGroup(0, r.renderGroup, [d * STRIDE]);
            pass.drawIndexedIndirect(r.args, d * 20);
        };
        // Nearer tiers first, so that they fill the depth before the farther ones shade. An impostor draw
        // without its layer draws the coarsest mesh (`aim`).
        for (let t = 0; t < TIERS - 1; t++) {
            pass.setPipeline(t < WHOLE_TIERS ? r.whole : r.mesh);
            this.specs.forEach((spec, m) => t < spec.tiers.length && indirect(m * TIERS + t));
        }
        this.specs.forEach((spec, m) => spec.impostor && !this.atlas.has(m) && indirect(m * TIERS + spec.tiers.length));
        if (!this.atlas.size) return;
        pass.setPipeline(r.impostor);
        pass.setBindGroup(1, r.atlasGroup);
        for (const [m] of this.atlas) indirect(m * TIERS + this.specs[m].tiers.length);
    }

    /** Gives a model a layer of the atlas, the layer that the frames drew least lately when none is free, and bakes it. */
    private wantImpostor(r: Resources, model: number, frame: number): void {
        const held = this.atlas.get(model);
        if (held) return void (held.used = frame);
        let slot = this.atlas.size < SLOTS ? this.atlas.size : -1;
        if (slot < 0) {
            const [oldest, entry] = [...this.atlas].reduce((a, b) => (b[1].used < a[1].used ? b : a));
            if (entry.used > frame - UNUSED_FRAMES) return;
            this.atlas.delete(oldest);
            slot = entry.slot;
        }
        this.atlas.set(model, { slot, used: frame });
        this.unbaked.add(model);
        const slots = new Uint32Array(MODELS);
        for (const [m, held] of this.atlas) slots[m] = held.slot;
        r.gpu.device.queue.writeBuffer(r.slots, 0, slots);
    }

    /** The outlines of a mesh's impostor (`outlines`), once per mesh and radius. */
    private outline(mesh: Mesh, radius: number): Float32Array<ArrayBuffer> {
        const known = this.outlined.get(mesh);
        if (known?.radius === radius) return known.outlines;
        const made = outlines(mesh, radius);
        this.outlined.set(mesh, { radius, outlines: made });
        return made;
    }

    /** Writes the models and their meshes into the pools; every impostor bakes again. */
    private upload(r: Resources): void {
        const models = new ArrayBuffer(MODELS * MODEL_BYTES), f = new Float32Array(models);
        const meshes = new Int32Array(DRAWS * MESH_BYTES / 4);
        let vertex = 0, index = 0;
        const fits = (tiers: Mesh[]) => {
            const count = tiers.reduce((sum, mesh) => sum + mesh.position.length / 3, 0), indices = tiers.reduce((sum, mesh) => sum + mesh.index.length, 0);
            return (vertex + count) * VERTEX_BYTES <= VERTEX_POOL && (index + indices) * 4 <= INDEX_POOL;
        };
        const put = (mesh: Mesh, extent: number): [number, number, number] => {
            const count = mesh.position.length / 3;
            r.gpu.device.queue.writeBuffer(r.vertices, vertex * VERTEX_BYTES, quantize(mesh, extent));
            r.gpu.device.queue.writeBuffer(r.indices, index * 4, Uint32Array.from(mesh.index));
            const at: [number, number, number] = [index, mesh.index.length, vertex];
            vertex += count;
            index += mesh.index.length;
            return at;
        };
        this.quad = put(QUAD, 1);
        // A mesh that models share, such as a tree as a tree and as a shrub, lies once in the pools.
        const shared = new Map<Mesh, { extent: number; at: [number, number, number] }>();
        const putOnce = (mesh: Mesh, extent: number) => {
            const known = shared.get(mesh);
            if (known?.extent === extent) return known.at;
            const at = put(mesh, extent);
            shared.set(mesh, { extent, at });
            return at;
        };
        const dropped: number[] = [];
        this.placed = this.specs.map((spec, m) => {
            const { tiers } = spec;
            if (!tiers.length) return [];
            if (!fits(tiers.map(({ mesh }) => mesh).filter(mesh => !shared.has(mesh)))) return void dropped.push(m), [];
            const extent = Math.max(...tiers.map(({ mesh }) => largest(mesh.position)));
            const placed = tiers.map(({ mesh, until }, t) => {
                const at = putOnce(mesh, extent);
                meshes.set([at[1], at[0], at[2], 0], (m * TIERS + t) * 4);
                f[m * 20 + t] = until;
                return at;
            });
            if (spec.impostor) {
                f[m * 20 + tiers.length] = spec.impostor.until;
                if (this.atlas.has(m)) this.unbaked.add(m);
            }
            f.set([extent, radius(spec), spec.fade ?? 0.15, spread(spec), spec.crown ?? -1, tiers.some(({ mesh }) => mesh.trunk) ? 1 : 0, spec.tie ?? 0, spec.sized ?? 0, ...sinks(spec)], m * 20 + 4);
            return placed;
        });
        // A model that does not fit the pools draws nothing; the frames go on.
        if (dropped.length) console.warn(`The instance meshes of ${dropped.length} models do not fit their pools.`);
        r.gpu.device.queue.writeBuffer(r.models, 0, models);
        // Per model the model of each phase (8 bits each, winter first) for the mesh tiers and for the
        // impostor tier, and its impostor tier + 1 for a model with phases, else 0; per model and phase
        // the colour matrix of its crown, the identity by default.
        const phases = new Uint32Array(MODELS * 4), tones = new Float32Array(MODELS * PHASES * TONE_BYTES / 4);
        for (let k = 0; k < MODELS * PHASES; k++) tones.set([1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0], 12 * k);
        this.specs.forEach((spec, m) => spec.phases?.forEach(({ model, impostor = model, tone }, phase) => {
            phases[4 * m] |= model << (8 * phase);
            phases[4 * m + 1] = spec.tiers.length + 1;
            phases[4 * m + 2] |= impostor << (8 * phase);
            if (tone) [0, 1, 2].forEach(column => tones.set(tone.slice(3 * column, 3 * column + 3), 12 * (impostor * PHASES + phase) + 4 * column));
        }));
        r.gpu.device.queue.writeBuffer(r.phases, 0, phases);
        r.gpu.device.queue.writeBuffer(r.tones, 0, tones);
        this.meshTable = meshes;
        r.gpu.device.queue.writeBuffer(r.meshes, 0, meshes);
        this.uploaded = true;
    }
}

/** Attaches a producer; one that fails logs why and draws nothing, and the rest go on. */
function attachProducer(producer: Producer, ...args: Parameters<Producer['attach']>): Promise<void> {
    return producer.attach(...args).catch(error => console.error('A producer of instances failed to attach:', error));
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

/**
 * Per frame of a model's impostor (8 × 8, FRAMES of hemi.wgsl), the trapezoid of the frame's square
 * (−1 to 1 over the sphere of `radius` about (0, 0, 0.5)) that holds the mesh seen along the frame and
 * along its eight neighbours: its bottom and top, and its half widths there, the least in area.
 */
export function outlines(mesh: Mesh, radius: number): Float32Array<ArrayBuffer> {
    const BANDS = 32, p = mesh.position, n = p.length / 3, own: number[][] = [];
    const xs = new Float64Array(n), ys = new Float64Array(n);
    for (let frame = 0; frame < FRAMES; frame++) {
        // hemi_decode and view_axes of hemi.wgsl.
        const ex = (frame % 8) / 3.5 - 1, ey = Math.floor(frame / 8) / 3.5 - 1, px = (ex + ey) / 2, py = (ex - ey) / 2;
        const pz = 1 - Math.abs(px) - Math.abs(py), norm = Math.hypot(px, py, pz), d = [px / norm, py / norm, pz / norm];
        const flat = Math.hypot(d[0], d[1]), right = Math.abs(d[2]) < 0.999 ? [-d[1] / flat, d[0] / flat, 0] : [1, 0, 0];
        const up = [d[1] * right[2] - d[2] * right[1], d[2] * right[0] - d[0] * right[2], d[0] * right[1] - d[1] * right[0]];
        let low = Infinity, high = -Infinity;
        for (let k = 0; k < n; k++) {
            const x = p[3 * k], y = p[3 * k + 1], z = p[3 * k + 2] - 0.5;
            xs[k] = Math.abs(x * right[0] + y * right[1] + z * right[2]) / radius;
            ys[k] = (x * up[0] + y * up[1] + z * up[2]) / radius;
            low = Math.min(low, ys[k]);
            high = Math.max(high, ys[k]);
        }
        // The widest |x| in each of BANDS bands of height, which the trapezoid holds at both ends of the band.
        const span = Math.max(high - low, 1e-6), widest = new Float64Array(BANDS).fill(-1);
        for (let k = 0; k < n; k++) {
            const band = Math.min(BANDS - 1, Math.floor(((ys[k] - low) / span) * BANDS));
            widest[band] = Math.max(widest[band], xs[k]);
        }
        // The least w0 + w1 where w0 (1 − t) + w1 t ≥ the widest at both ends t of every band: convex in w1.
        const bottom = (top: number) => {
            let w = 0;
            for (let band = 0; band < BANDS; band++) {
                const x = widest[band];
                if (x < 0) continue;
                // At the band's foot; its top binds harder unless it is the trapezoid's top.
                w = Math.max(w, (x - (top * band) / BANDS) / (1 - band / BANDS));
                if (band < BANDS - 1) w = Math.max(w, (x - (top * (band + 1)) / BANDS) / (1 - (band + 1) / BANDS));
            }
            return w;
        };
        // The top band's top needs w1 at least its widest.
        let a = widest[BANDS - 1], b = Math.max(...widest);
        for (let step = 0; step < 24; step++) {
            const m1 = a + (b - a) / 3, m2 = b - (b - a) / 3;
            if (m1 + bottom(m1) <= m2 + bottom(m2)) b = m2;
            else a = m1;
        }
        own.push([low, high, bottom(b) + 1e-4, b + 1e-4]);
    }
    // A trapezoid holds its neighbours' where its edges hold their widest at its bottom and top: the
    // widest of lines is convex, so it lies under the chord between those.
    const out = new Float32Array(FRAMES * 4);
    for (let frame = 0; frame < FRAMES; frame++) {
        const x = frame % 8, y = Math.floor(frame / 8), near: number[][] = [];
        for (let j = Math.max(0, y - 1); j <= Math.min(7, y + 1); j++) for (let i = Math.max(0, x - 1); i <= Math.min(7, x + 1); i++) near.push(own[8 * j + i]);
        const low = Math.min(...near.map(o => o[0])), high = Math.max(...near.map(o => o[1]));
        const width = (at: number) => Math.max(0, ...near.map(([l, h, w0, w1]) => w0 + ((w1 - w0) * (at - l)) / Math.max(h - l, 1e-6)));
        out.set([low, high, width(low), width(high)], 4 * frame);
    }
    return out;
}

/** The farthest that a vertex of a model lies from its axis. */
function spread(spec: ModelSpec): number {
    let most = 0;
    for (const { mesh: { position: p } } of spec.tiers) for (let i = 0; i < p.length; i += 3) most = Math.max(most, Math.hypot(p[i], p[i + 1]));
    return most;
}

/**
 * A tree's sink table (slope_sink of emit.wgsl): per slope SINK_STEP × (k + 1), the most that the drop
 * of the ground under a crown vertex, the slope × its reach, exceeds its height above the crown's base;
 * a vertex below the base counts as at it. Over the vertices of every tier but their trunks'.
 */
export function sinks(spec: ModelSpec): number[] {
    const out = new Array<number>(SINK_SLOPES).fill(0), crown = spec.crown;
    if (crown === undefined) return out;
    for (const { mesh: { position: p, trunk = 0 } } of spec.tiers)
        for (let i = 3 * trunk; i < p.length; i += 3) {
            const reach = Math.hypot(p[i], p[i + 1]), above = Math.max(0, p[i + 2] - crown);
            for (let k = 0; k < SINK_SLOPES; k++) out[k] = Math.max(out[k], SINK_STEP * (k + 1) * reach - above);
        }
    return out;
}

/**
 * The vertices of a mesh as the instance pipeline reads them, 16 bytes each: the position over
 * `extent` as snorm16 × 3 and, for a vertex of the trunk, the trunk's top in model units (else 0); the
 * normal as snorm8 × 4 and the colour in sRGB as unorm8 × 4.
 */
function quantize(mesh: Mesh, extent: number): ArrayBuffer {
    const count = mesh.position.length / 3, out = new ArrayBuffer(count * VERTEX_BYTES);
    const i16 = new Int16Array(out), i8 = new Int8Array(out), u8 = new Uint8Array(out), trunk = mesh.trunk ?? 0;
    let top = 0;
    for (let k = 0; k < trunk; k++) top = Math.max(top, mesh.position[3 * k + 2]);
    const srgb = (c: number) => Math.round(255 * Math.min(1, Math.max(0, c <= 0.0031308 ? 12.92 * c : 1.055 * c ** (1 / 2.4) - 0.055)));
    for (let k = 0; k < count; k++) {
        for (let d = 0; d < 3; d++) {
            i16[8 * k + d] = Math.round(32767 * mesh.position[3 * k + d] / extent);
            i8[16 * k + 8 + d] = Math.round(127 * (mesh.normal[3 * k + d] ?? 0));
            u8[16 * k + 12 + d] = srgb(mesh.color[3 * k + d] ?? 1);
        }
        i16[8 * k + 3] = k < trunk ? Math.max(1, Math.round(32767 * Math.min(1, top))) : 0;
        u8[16 * k + 15] = 255;
    }
    return out;
}
