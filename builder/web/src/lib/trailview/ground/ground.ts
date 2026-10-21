import { EARTH_RADIUS, tileLonLat } from '../frame/frame';
import type { Gpu } from '../gpu/device';
import { wgsl } from '../gpu/wgsl';
import { LOOK_CODES } from '../light/look';
import type { Labs } from '../labs';
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

/** Without a ruleset, the ground is the palette under the light and haze of the prototype's `world` region. */
const NEUTRAL: Rules = {
    vocabulary: { materials: [{ id: 'none', cover: 'grass' }], lithologies: [{ id: 'unknown', hardness: 0.5 }] },
    regions: [{
        id: 'world', chain: ['world'], leaf: 'mixed', look: {
            snap: 0.4, hazeKm: 35, hazeHeight: 2500, mistTop: 0, softness: 0.08, gradient: 0.35, ramps: {}, outcrops: 0, snow_share: 0.5, turf_deg: 42,
            rock: '#8c877d', sea: { deep: '#2c5874', shallow: '#4f8088' }, grade: { shadow: [-0.004, 0, 0.012], light: [0.01, 0.004, -0.008], saturation: 1.08 },
        },
    }],
};

export interface GroundStats { resident: number; capacity: number; wanted: number; shortage: number; colour: string; requests: number; bytes: number; failed: number }

interface Resources {
    gpu: Gpu;
    cells: GPUTexture;
    colour: GPUTexture;
    snow: GPUTexture;
    openness: GPUTexture;
    cache: GPUTexture;
    rules: GPUTexture;
    map: GPUBuffer;
    jobs: GPUBuffer;
    open: GPUBuffer;
    group: GPUBindGroup;
    computeGroups: [GPUBindGroup, GPUBindGroup];
    compute: GPUComputePipeline;
}

/**
 * The ground pages on the GPU: the textures of the pool of `GroundPages`, the ground map, the rules of
 * the date and the compute of the openness and the cache. A new date builds the pages again; they draw
 * the old date until then.
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
    private jobs: { page: Slot; work: number }[] = [];

    constructor(private readonly source: GroundSource | undefined, private readonly labs: Labs, changed: () => void) {
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

    get stats(): GroundStats {
        const { requests = 0, bytes = 0, failed = 0 } = this.source ?? {}, { wanted, shortage } = this.pages;
        return { resident: this.pages.resident, capacity: POOL, wanted, shortage, colour: this.source?.colour ?? 'none', requests, bytes, failed };
    }

    get attribution(): string[] {
        return this.source?.attribution ?? [];
    }

    /** The bind group layouts: group 2 of the terrain and the sky, and groups 0 and 2 of the ground compute. */
    layouts(device: GPUDevice): { render: GPUBindGroupLayout; compute: GPUBindGroupLayout[] } {
        const stages = GPUShaderStage.FRAGMENT | GPUShaderStage.COMPUTE;
        const entries: GPUBindGroupLayoutEntry[] = [
            { binding: 0, visibility: stages, buffer: { type: 'uniform' } },
            { binding: 1, visibility: stages, buffer: { type: 'read-only-storage' } },
            { binding: 2, visibility: stages, texture: { sampleType: 'uint', viewDimension: '2d-array' } },
            ...[3, 4, 5].map(binding => ({ binding, visibility: stages, texture: { sampleType: 'float', viewDimension: '2d-array' } }) as GPUBindGroupLayoutEntry),
            { binding: 6, visibility: stages, texture: { sampleType: 'unfilterable-float' } },
            { binding: 7, visibility: stages, sampler: {} },
        ];
        const render = device.createBindGroupLayout({ entries: [...entries, { binding: 8, visibility: GPUShaderStage.FRAGMENT, texture: { sampleType: 'float', viewDimension: '2d-array' } }] });
        // The compute writes the cache: its bind group does not hold it for sampling.
        const compute = [
            device.createBindGroupLayout({ entries: [
                { binding: 0, visibility: GPUShaderStage.COMPUTE, buffer: { type: 'read-only-storage' } },
                { binding: 1, visibility: GPUShaderStage.COMPUTE, buffer: { type: 'storage' } },
                { binding: 2, visibility: GPUShaderStage.COMPUTE, storageTexture: { format: 'rgba8unorm', viewDimension: '2d-array' } },
            ] }),
            device.createBindGroupLayout({ entries }),
        ];
        this.layout = { render, compute };
        return this.layout;
    }

    /** The pools and the compute for a device, after `layouts`. A new device builds every page again. */
    async attach(gpu: Gpu, look: GPUBuffer, surface: { layout: GPUBindGroupLayout; group: GPUBindGroup }): Promise<void> {
        const { device } = gpu, layout = this.layout!, layers = POOL / LAYER_PAGES, side = PAGE * SIDE, cache = this.labs.ground === 'cache';
        this.resources = undefined;
        this.lost();
        // The compute of the cache reads the codes, and a page can arrive before the first frame with an eye writes the look.
        device.queue.writeBuffer(look, LOOK_CODES, Uint32Array.from(this.codes));
        const usage = GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST;
        const texture = (format: GPUTextureFormat, bytes: number, depth = layers, size = side, extra = 0) =>
            gpu.texture('ground', { size: [size, size, depth], format, usage: usage | extra }, size * size * depth * bytes);
        const SLOT = slots(this.rules.vocabulary);
        const r = {
            gpu,
            cells: texture('rgba8uint', 4), colour: texture('rgba8unorm', 4), snow: texture('rgba8unorm', 4), openness: texture('r8unorm', 1),
            cache: cache ? texture('rgba8unorm', 4, 4 * layers, side, GPUTextureUsage.STORAGE_BINDING) : texture('rgba8unorm', 4, 4, 1, GPUTextureUsage.STORAGE_BINDING),
            rules: gpu.texture('ground', { size: [SLOT.count, this.rules.regions.length], format: 'rgba32float', usage }, SLOT.count * this.rules.regions.length * 16),
            map: gpu.buffer('ground', (this.finest + 1) * MAP * MAP * 4, GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST),
            jobs: gpu.buffer('compute', JOBS * JOB_BYTES, GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST),
            open: gpu.buffer('compute', JOBS * PAGE * PAGE, GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC),
        };
        const sampler = device.createSampler({ magFilter: 'linear', minFilter: 'linear' });
        const view = (t: GPUTexture) => t.createView({ dimension: '2d-array' });
        const inputs: GPUBindGroupEntry[] = [
            { binding: 0, resource: { buffer: look } }, { binding: 1, resource: { buffer: r.map } }, { binding: 2, resource: view(r.cells) },
            { binding: 3, resource: view(r.colour) }, { binding: 4, resource: view(r.snow) }, { binding: 5, resource: view(r.openness) },
            { binding: 6, resource: r.rules.createView() }, { binding: 7, resource: sampler },
        ];
        const module = device.createShaderModule({ label: 'ground', code: wgsl('ground/ground.wgsl') });
        const compute = await device.createComputePipelineAsync({
            label: 'ground', layout: device.createPipelineLayout({ bindGroupLayouts: [layout.compute[0], surface.layout, layout.compute[1]] }),
            compute: { module, entryPoint: 'ground_main', constants: surfaceConstants(this.labs) },
        });
        this.resources = {
            ...r, compute,
            group: device.createBindGroup({ layout: layout.render, entries: [...inputs, { binding: 8, resource: view(r.cache) }] }),
            computeGroups: [
                device.createBindGroup({ layout: layout.compute[0], entries: [
                    { binding: 0, resource: { buffer: r.jobs } }, { binding: 1, resource: { buffer: r.open } }, { binding: 2, resource: view(r.cache) },
                ] }),
                device.createBindGroup({ layout: layout.compute[1], entries: inputs }),
            ],
        };
        this.surfaceGroup = surface.group;
    }

    /** Group 2 of the terrain and the sky. */
    get group(): GPUBindGroup | undefined {
        return this.resources?.group;
    }

    /**
     * Takes the pages that the drawn nodes and the selection want about the eye (lon, lat), on the date
     * of the view. `refreshed` are the nodes whose heights changed, so their openness follows.
     */
    update(drawn: Node[], wanted: Node[], refreshed: Node[], origins: [number, number][], date: string, eye: [number, number]): void {
        const r = this.resources;
        if (!r) return;
        if (this.pages.setDate(date)) {
            const SLOT = slots(this.rules.vocabulary), rows = this.rules.regions.length;
            r.gpu.device.queue.writeTexture({ texture: r.rules }, packRulesOn(this.rules.regions, this.rules.vocabulary, this.pages.day), { bytesPerRow: SLOT.count * 16 }, [SLOT.count, rows]);
        }
        const work = this.pages.update(drawn, wanted, refreshed, origins, eye);
        for (const { page, data } of work.uploads) {
            this.upload(r, page, data);
            this.jobs.push({ page, work: this.labs.ground === 'cache' ? 3 : 1 });
        }
        for (const page of work.refreshes) this.jobs.push({ page, work: 1 });
        if (work.map) r.gpu.device.queue.writeBuffer(r.map, 0, work.map);
    }

    /** Encodes the openness and the cache of the pages that arrived or whose heights changed, timed by `timing` when it runs. */
    encode(encoder: GPUCommandEncoder, timing?: () => GPUComputePassTimestampWrites | undefined): void {
        const r = this.resources, jobs = this.jobs.splice(0, JOBS);
        if (!r || !jobs.length || !this.surfaceGroup) return;
        const data = new ArrayBuffer(jobs.length * JOB_BYTES), ints = new Int32Array(data), floats = new Float32Array(data);
        jobs.forEach(({ page: { node: { level, x, y }, slot }, work }, k) => {
            ints.set([x * PAGE, y * PAGE, level, slot], 8 * k);
            floats[8 * k + 4] = 2 * Math.PI * EARTH_RADIUS * Math.cos(tileLonLat(x + 0.5, y + 0.5, level)[1] * RAD) / 2 ** (level + 8);
            ints[8 * k + 5] = work;
        });
        r.gpu.device.queue.writeBuffer(r.jobs, 0, data);
        const pass = encoder.beginComputePass({ label: 'ground', timestampWrites: timing?.() });
        pass.setPipeline(r.compute);
        pass.setBindGroup(0, r.computeGroups[0]);
        pass.setBindGroup(1, this.surfaceGroup);
        pass.setBindGroup(2, r.computeGroups[1]);
        pass.dispatchWorkgroups(PAGE / 4 / 16, PAGE / 16, jobs.length);
        pass.end();
        jobs.forEach(({ page }, k) => encoder.copyBufferToTexture({ buffer: r.open, offset: k * PAGE * PAGE, bytesPerRow: PAGE }, { texture: r.openness, origin: origin(page.slot) }, [PAGE, PAGE, 1]));
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

    private upload(r: Resources, page: Slot, data: GroundPage): void {
        const { device } = r.gpu;
        for (const [texture, texels] of [[r.cells, data.cells], [r.colour, data.colour], [r.snow, data.snow]] as const) {
            device.queue.writeTexture({ texture, origin: origin(page.slot) }, texels, { bytesPerRow: 4 * PAGE }, [PAGE, PAGE, 1]);
        }
    }
}

/** The overrides of drawn-height.wgsl, as the terrain sets them. */
export const surfaceConstants = (labs: Labs) => ({ RINGS: labs.terrain === 'rings' ? 1 : 0, R16: labs.pages === 'r16' ? 1 : 0 });

/** Where a ground page lies: one of SIDE × SIDE tiles of a layer, as `in_page` in look.wgsl. */
const origin = (slot: number): [number, number, number] => [PAGE * (slot % SIDE), PAGE * (Math.floor(slot / SIDE) % SIDE), Math.floor(slot / LAYER_PAGES)];
