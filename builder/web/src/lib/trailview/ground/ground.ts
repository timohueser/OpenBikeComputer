import { EARTH_RADIUS, tileLonLat } from '../frame/frame';
import type { Gpu } from '../gpu/device';
import { wgsl } from '../gpu/wgsl';
import { LOOK_CODES } from '../light/look';
import type { Region, Rules } from '../rules/regions';
import type { GroundSource } from '../stream/ground';
import type { GroundPage } from '../stream/ground-pages';
import { PAGE } from '../stream/heights';
import { COARSEST, MAP, type Node } from '../terrain/lod';
import { GroundPages, type Slot } from './pages';
import { packRulesOn, slots } from './rules';

/** Ground pages: whole layers of SIDE × SIDE pages, as GROUND_SIDE in look.wgsl. */
const SIDE = 4, LAYER_PAGES = SIDE * SIDE, POOL = 128;
/** Compute jobs per frame at most: the pages that arrive and the pages whose heights changed, as `GroundPages` takes them. */
const JOBS = 16, JOB_BYTES = 32;
const RAD = Math.PI / 180;
/** Texels a side of the noise table, as in look.wgsl. */
const NOISE = 256;

/** Without a ruleset, the ground is the palette under the light and haze of the prototype's `world` region. */
const NEUTRAL: Rules = {
    vocabulary: { materials: [{ id: 'none', cover: 'grass' }], lithologies: [{ id: 'unknown', hardness: 0.5 }] },
    regions: [{
        id: 'world', chain: ['world'], leaf: 'mixed', look: {
            snap: 0.4, hazeKm: 35, hazeHeight: 2500, mistTop: 0, softness: 0.08, gradient: 0.35, ramps: {}, outcrops: 0, snow_share: 0.5, turf_deg: 42,
            rock: '#8c877d', sea: { deep: '#2c5874', shallow: '#4f8088' }, grade: { shadow: [-0.004, 0, 0.012], light: [0.01, 0.004, -0.008], saturation: 1.08 },
        },
        houses: [0, 0],
        trees: [],
        ground: [],
    }],
    styles: [],
};

export interface GroundStats { resident: number; capacity: number; wanted: number; shortage: number; colour: string; requests: number; bytes: number; failed: number }

interface Resources {
    gpu: Gpu;
    cells: GPUTexture;
    cache: GPUTexture;
    /** The colour and the snow of the pages that a frame's jobs make, one layer per job. */
    colour: GPUTexture;
    snow: GPUTexture;
    rules: GPUTexture;
    noise: GPUTexture;
    map: GPUBuffer;
    jobs: GPUBuffer;
    group: GPUBindGroup;
    computeGroups: [GPUBindGroup, GPUBindGroup];
    compute: GPUComputePipeline;
}

/**
 * The ground pages on the GPU: the textures of the pool of `GroundPages`, the ground map, the rules of
 * the date, and the compute that makes the ground parameters and the relief of a page. A new date
 * builds the pages again; they draw the old date until then.
 */
export class Ground {
    readonly rules: Rules;
    readonly finest: number;
    /** `codes` of the look: the material code of heath, the finest level, and the sizes of the vocabulary. */
    readonly codes: [number, number, number, number];
    private layout?: { render: GPUBindGroupLayout; compute: GPUBindGroupLayout[] };
    private resources?: Resources;
    private surfaceGroup?: GPUBindGroup;
    private readonly pages: GroundPages;
    private jobs: { page: Slot; data?: GroundPage }[] = [];
    /** Whether the last frame draws another ground than the frame before it: new pages, a new map or a new date. */
    newContent = false;

    constructor(private readonly source: GroundSource | undefined, changed: () => void) {
        this.rules = source?.rules ?? NEUTRAL;
        this.finest = source?.finest ?? COARSEST;
        const { materials, lithologies } = this.rules.vocabulary;
        this.codes = [Math.max(0, materials.findIndex(material => material.id === 'heath')), this.finest, materials.length, lithologies.length];
        const world = Math.max(0, this.rules.regions.findIndex(region => region.id === 'world'));
        this.pages = new GroundPages(source, POOL, this.finest, this.rules.regions.map((region: Region) => region.look.snow_share), world, changed);
    }

    get idle(): boolean {
        return this.pages.idle && this.jobs.length === 0;
    }

    /** Whether the next frame has work that no load in flight wakes. */
    get working(): boolean {
        return this.pages.working || this.jobs.length > 0;
    }

    /** The terrain nodes that the relief of the ground reads: the terrain keeps them resident, so the relief does not hang on what the view happens to load. */
    get reads(): Node[] {
        return this.pages.reads;
    }

    get stats(): GroundStats {
        const { requests = 0, bytes = 0, failed = 0 } = this.source ?? {}, { wanted, shortage } = this.pages;
        return { resident: this.pages.resident, capacity: POOL, wanted, shortage, colour: this.source?.colour ?? 'none', requests, bytes, failed };
    }

    get attribution(): string[] {
        return this.source?.attribution ?? [];
    }

    /** The bind group layouts: group 2 of the terrain and the sky, and groups 0 and 2 of the ground compute. */
    layouts(device: GPUDevice): { render: GPUBindGroupLayout; compute: GPUBindGroupLayout[] } {
        // The instances' vertices read the look for the haze.
        const stages = GPUShaderStage.VERTEX | GPUShaderStage.FRAGMENT | GPUShaderStage.COMPUTE, compute = GPUShaderStage.COMPUTE;
        const array = (sampleType: GPUTextureSampleType): Omit<GPUBindGroupLayoutEntry, 'binding' | 'visibility'> => ({ texture: { sampleType, viewDimension: '2d-array' } });
        const entries: GPUBindGroupLayoutEntry[] = [
            { binding: 0, visibility: stages, buffer: { type: 'uniform' } },
            { binding: 1, visibility: stages, buffer: { type: 'read-only-storage' } },
            { binding: 2, visibility: stages, ...array('uint') },
            { binding: 3, visibility: stages, texture: { sampleType: 'unfilterable-float' } },
            { binding: 4, visibility: stages, sampler: {} },
        ];
        // The compute writes the cache: its bind group does not hold it for sampling. The producers of
        // instances read the snow in it.
        this.layout = {
            render: device.createBindGroupLayout({ entries: [
                ...entries, { binding: 5, visibility: GPUShaderStage.FRAGMENT | compute, ...array('float') }, { binding: 6, visibility: GPUShaderStage.FRAGMENT, texture: { sampleType: 'float' } },
            ] }),
            compute: [
                device.createBindGroupLayout({ entries: [
                    { binding: 0, visibility: compute, buffer: { type: 'read-only-storage' } },
                    { binding: 1, visibility: compute, storageTexture: { format: 'rgba8unorm', viewDimension: '2d-array' } },
                    { binding: 2, visibility: compute, ...array('float') },
                    { binding: 3, visibility: compute, ...array('float') },
                ] }),
                device.createBindGroupLayout({ entries }),
            ],
        };
        return this.layout;
    }

    /** The pools and the compute for a device, after `layouts`. A new device builds every page again. */
    async attach(gpu: Gpu, look: GPUBuffer, surface: { layout: GPUBindGroupLayout; group: GPUBindGroup }): Promise<void> {
        const { device } = gpu, layout = this.layout!, layers = POOL / LAYER_PAGES, side = PAGE * SIDE;
        this.resources = undefined;
        this.lost();
        // The compute reads the codes, and a page can arrive before the first frame with an eye writes the look.
        device.queue.writeBuffer(look, LOOK_CODES, Uint32Array.from(this.codes));
        const usage = GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST;
        const texture = (pool: string, format: GPUTextureFormat, size: number, depth: number, usage: number) =>
            gpu.texture(pool, { size: [size, size, depth], format, usage }, size * size * depth * 4);
        const SLOT = slots(this.rules.vocabulary);
        const r = {
            gpu,
            cells: texture('ground', 'rgba8uint', side, layers, usage),
            cache: texture('ground', 'rgba8unorm', side, 4 * layers, GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.STORAGE_BINDING),
            colour: texture('compute', 'rgba8unorm', PAGE, JOBS, usage), snow: texture('compute', 'rgba8unorm', PAGE, JOBS, usage),
            rules: gpu.texture('ground', { size: [SLOT.count, this.rules.regions.length], format: 'rgba32float', usage }, SLOT.count * this.rules.regions.length * 16),
            noise: gpu.texture('ground', { size: [NOISE, NOISE], format: 'r8unorm', usage }, NOISE * NOISE),
            map: gpu.buffer('ground', (this.finest + 1) * MAP * MAP * 4, GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST),
            jobs: gpu.buffer('compute', JOBS * JOB_BYTES, GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST),
        };
        device.queue.writeTexture({ texture: r.noise }, noiseTable(), { bytesPerRow: NOISE }, [NOISE, NOISE]);
        // It repeats for the noise table; a cache read stays inside its page.
        const sampler = device.createSampler({ magFilter: 'linear', minFilter: 'linear', addressModeU: 'repeat', addressModeV: 'repeat' });
        const view = (t: GPUTexture) => t.createView({ dimension: '2d-array' });
        const inputs: GPUBindGroupEntry[] = [
            { binding: 0, resource: { buffer: look } }, { binding: 1, resource: { buffer: r.map } }, { binding: 2, resource: view(r.cells) },
            { binding: 3, resource: r.rules.createView() }, { binding: 4, resource: sampler },
        ];
        const module = device.createShaderModule({ label: 'ground', code: wgsl('ground/ground.wgsl') });
        const compute = await device.createComputePipelineAsync({
            label: 'ground', layout: device.createPipelineLayout({ bindGroupLayouts: [layout.compute[0], surface.layout, layout.compute[1]] }),
            compute: { module, entryPoint: 'ground_main' },
        });
        this.resources = {
            ...r, compute,
            group: device.createBindGroup({ layout: layout.render, entries: [...inputs, { binding: 5, resource: view(r.cache) }, { binding: 6, resource: r.noise.createView() }] }),
            computeGroups: [
                device.createBindGroup({ layout: layout.compute[0], entries: [
                    { binding: 0, resource: { buffer: r.jobs } }, { binding: 1, resource: view(r.cache) }, { binding: 2, resource: view(r.colour) }, { binding: 3, resource: view(r.snow) },
                ] }),
                device.createBindGroup({ layout: layout.compute[1], entries: inputs }),
            ],
        };
        this.surfaceGroup = surface.group;
    }

    /** Group 2 of a compute pass that reads the ground pages, with the layout `layouts().compute[1]`. */
    get computeGroup(): GPUBindGroup | undefined {
        return this.resources?.computeGroups[1];
    }

    /** Group 2 of the terrain and the sky. */
    get group(): GPUBindGroup | undefined {
        return this.resources?.group;
    }

    /**
     * Takes the pages that the drawn nodes and the selection want about the eye (lon, lat), on the date
     * of the view. `refreshed` are the terrain nodes whose heights changed, so the relief that reads them follows.
     */
    update(drawn: Node[], wanted: Node[], refreshed: Node[], origins: [number, number][], date: string, eye: [number, number]): void {
        const r = this.resources;
        if (!r) return;
        const day = this.pages.setDate(date);
        if (day) {
            const SLOT = slots(this.rules.vocabulary), rows = this.rules.regions.length;
            r.gpu.device.queue.writeTexture({ texture: r.rules }, packRulesOn(this.rules.regions, this.rules.vocabulary, this.pages.day), { bytesPerRow: SLOT.count * 16 }, [SLOT.count, rows]);
        }
        const work = this.pages.update(drawn, wanted, refreshed, origins, eye);
        for (const { page, data } of work.uploads) this.jobs.push({ page, data });
        for (const page of work.refreshes) this.jobs.push({ page });
        if (work.map) r.gpu.device.queue.writeBuffer(r.map, 0, work.map);
        this.newContent = day || !!work.map || this.jobs.length > 0;
    }

    /**
     * Encodes the ground parameters and the relief of the pages that arrived, and the relief of the
     * pages whose heights changed, timed by `timing` when it runs.
     */
    encode(encoder: GPUCommandEncoder, timing?: () => GPUComputePassTimestampWrites | undefined): void {
        const r = this.resources, jobs = this.jobs.splice(0, JOBS);
        if (!r || !jobs.length || !this.surfaceGroup) return;
        const data = new ArrayBuffer(jobs.length * JOB_BYTES), ints = new Int32Array(data), floats = new Float32Array(data);
        jobs.forEach(({ page: { node: { level, x, y }, slot }, data }, k) => {
            ints.set([x * PAGE, y * PAGE, level, slot], 8 * k);
            floats[8 * k + 4] = 2 * Math.PI * EARTH_RADIUS * Math.cos(tileLonLat(x + 0.5, y + 0.5, level)[1] * RAD) / 2 ** (level + 8);
            ints[8 * k + 5] = data ? 3 : 1;
            if (data) this.upload(r, slot, k, data);
        });
        r.gpu.device.queue.writeBuffer(r.jobs, 0, data);
        const pass = encoder.beginComputePass({ label: 'ground', timestampWrites: timing?.() });
        pass.setPipeline(r.compute);
        pass.setBindGroup(0, r.computeGroups[0]);
        pass.setBindGroup(1, this.surfaceGroup);
        pass.setBindGroup(2, r.computeGroups[1]);
        pass.dispatchWorkgroups(PAGE / 16, PAGE / 16, jobs.length);
        pass.end();
    }

    /** A lost device keeps the tile bodies; every page comes again. */
    lost(): void {
        this.pages.lost();
        this.jobs = [];
    }

    /** The regions about a point and their weights, which sum to 1. */
    regionsAt(lon: number, lat: number): Map<number, number> {
        return this.pages.regionsAt(lon, lat);
    }

    /** The cells of a page into its slot, and its colour and snow into layer `job` of the inputs of the compute. */
    private upload(r: Resources, slot: number, job: number, data: GroundPage): void {
        const write = (texture: GPUTexture, at: [number, number, number], texels: Uint8Array<ArrayBuffer>) =>
            r.gpu.device.queue.writeTexture({ texture, origin: at }, texels, { bytesPerRow: 4 * PAGE }, [PAGE, PAGE, 1]);
        write(r.cells, origin(slot), data.cells);
        write(r.colour, [0, 0, job], data.colour);
        write(r.snow, [0, 0, job], data.snow);
    }
}

/** Where a ground page lies: one of SIDE × SIDE tiles of a layer, as `in_page` in look.wgsl. */
const origin = (slot: number): [number, number, number] => [PAGE * (slot % SIDE), PAGE * (Math.floor(slot / SIDE) % SIDE), Math.floor(slot / LAYER_PAGES)];

/** The noise table of look.wgsl: one hash byte per texel, the same on every run. */
function noiseTable(): Uint8Array<ArrayBuffer> {
    const table = new Uint8Array(NOISE * NOISE);
    let x = 0x9e3779b9;
    for (let i = 0; i < table.length; i++) {
        x ^= x << 13;
        x ^= x >>> 17;
        x ^= x << 5;
        table[i] = x & 0xff;
    }
    return table;
}
