import { worldPixel } from '../../planner/layers/mercator';
import { anchorAt, UNITS } from '../frame/space';
import type { Gpu } from '../gpu/device';
import { layouts } from '../gpu/layouts';
import { wgsl } from '../gpu/wgsl';
import { Instances, VIEWS, type ModelSpec, type Producer, type Surfaces } from '../instances/instances';
import type { ModelFiles } from '../instances/model-files';
import type { Packed } from '../models/model-file';
import { bits, named, type Rules } from '../rules/regions';

/** Herds stand within this many metres, rocks within this many; as HERDS of dressing.wgsl and ROCK_REACH_M of rocks.wgsl. */
const HERD_REACH_M = 1500, ROCK_REACH_M = 1000, HERDS = 4;
/** Units along a herd cell and a rock point's cell, as HERD_SHIFT of dressing.wgsl and ROCK_SHIFT of rocks.wgsl. */
const HERD_CELL = 2 ** 14, ROCK_CELL = 2 ** 7;
/** Words of `Herd` and bytes of `Dressing` in dressing.wgsl. */
const HERD_WORDS = 8, DRESSING_BYTES = 64;

/**
 * Prototype #3786: `showroom=lat,lon,heading,breed|breed|…` stands the breeds in a row across a view toward `heading`
 * (degrees from north) at (lat, lon), SHOW_SPACING_M apart, each facing right and turned SHOW_TURN toward the eye.
 */
const SHOW_SPACING_M = 3.2, SHOW_TURN = 0.45;
function showroom(): { lonLat: [number, number]; heading: number; breeds: string[] } | undefined {
    const value = globalThis.location ? new URLSearchParams(globalThis.location.search).get('showroom') : null;
    if (!value) return undefined;
    const [lat, lon, heading, breeds] = value.split(',');
    return { lonLat: [Number(lon), Number(lat)], heading: (Number(heading) * Math.PI) / 180, breeds: breeds.split('|') };
}

interface Resources { gpu: Gpu; herds: GPUComputePipeline; rocks: GPUComputePipeline; groups: GPUBindGroup[]; params: GPUBuffer }

/**
 * The dressing of the ground (dressing.wgsl): the herds of the regions' packs on the materials that
 * they graze, and rocks on rough and rocky ground, placed on the GPU each frame. Their models come from
 * model files (models/model-file.ts): the rocks from the objects file, each breed from a file of its own.
 */
export class Dressing implements Producer {
    private resources?: Resources;
    private readonly table: Uint32Array<ArrayBuffer>;
    private readonly rockModel?: number;
    private herdSide = 0;
    private rockSide = 0;
    /** Prototype #3786: the showroom, and the first of its entries in the table. */
    private readonly show = showroom();
    private showFirst = 0;
    /** The files of the models. */
    private readonly urls: string[] = [];

    constructor(rules: Rules, private readonly instances: Instances, private readonly files: ModelFiles) {
        instances.produce(this);
        const { objects, breeds: breedModels } = files.models;
        const spec = ({ tiers: [mesh], shape }: Packed, until: number): ModelSpec => ({ tiers: [{ mesh, until }], shape });
        // No fade of the rocks' tier: rocks_of ends each rock at its own distance before the reach.
        this.rockModel = this.load(objects.file, objects.rocks, model => ({ ...spec(model, ROCK_REACH_M), fade: 0 }));
        const breeds = new Map<string, number>(), used = new Set([...rules.regions.flatMap(region => region.herds.map(herd => herd.model)), ...(this.show?.breeds ?? [])].filter(name => name in breedModels));
        for (const name of used) {
            const first = this.load(breedModels[name].file, breedModels[name].coats, model => spec(model, HERD_REACH_M));
            if (first !== undefined) breeds.set(name, first);
        }
        this.showFirst = Math.max(1, rules.regions.length) * HERDS;
        this.table = new Uint32Array((this.showFirst + (this.show?.breeds.length ?? 0)) * HERD_WORDS);
        const floats = new Float32Array(this.table.buffer);
        // The row runs across the view to its right; each animal faces right, turned toward the eye.
        this.show?.breeds.forEach((name, k) => {
            const at = (this.showFirst + k) * HERD_WORDS, view = Math.PI / 2 - this.show!.heading, across = view - Math.PI / 2;
            const offset = (k - (this.show!.breeds.length - 1) / 2) * SHOW_SPACING_M;
            this.table.set([breeds.get(name) ?? 0, 1], at);
            floats.set([across - Math.PI / 2 - SHOW_TURN, breedModels[name]?.height ?? 1, offset * Math.cos(across), offset * Math.sin(across)], at + 2);
        });
        rules.regions.forEach((region, r) => {
            region.herds.filter(herd => breeds.has(herd.model)).slice(0, HERDS).forEach((herd, k) => {
                const at = (r * HERDS + k) * HERD_WORDS;
                this.table.set([breeds.get(herd.model)!, breedModels[herd.model].coats], at);
                floats.set([herd.groups, breedModels[herd.model].height, ...herd.size], at + 2);
                this.table.set(bits(named(rules.vocabulary, herd.on)), at + 6);
            });
        });
    }

    /** The first of `count` models of the instances, which take the first `count` models of a file once it arrives. */
    private load(file: string, count: number, spec: (model: Packed) => ModelSpec): number | undefined {
        const first = this.instances.add(...Array.from({ length: count }, () => ({ tiers: [] })));
        this.urls.push(file);
        void this.files.load(file).then(models => {
            if (first === undefined || !models) return;
            for (let k = 0; k < count; k++) this.instances.replace(first + k, spec(models[k]));
        });
        return first;
    }

    /** Whether the models have arrived, or failed. */
    get idle(): boolean {
        return this.files.done(...this.urls);
    }

    async attach(gpu: Gpu, { surface, features }: Surfaces): Promise<void> {
        this.resources = undefined;
        const { device } = gpu;
        const herds = gpu.buffer('dressing', this.table.byteLength, GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST);
        const params = gpu.buffer('dressing', DRESSING_BYTES, GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST);
        device.queue.writeBuffer(herds, 0, this.table);
        const [herdsOf, rocksOf] = await gpu.once(dressingPipelines);
        this.resources = {
            gpu, herds: herdsOf, rocks: rocksOf, params,
            groups: VIEWS.map(view => device.createBindGroup({ layout: herdsOf.getBindGroupLayout(0), entries: [...this.instances.emitResources(view), { binding: 8, resource: { buffer: params } }, { binding: 9, resource: { buffer: herds } }] })),
        };
        this.surface = surface;
        this.features = features;
    }

    private surface?: GPUBindGroup;
    private features?: GPUBindGroup;

    /** The windows of herd cells and rock points about the eye at (lon, lat). */
    update(lonLat: [number, number]): void {
        const r = this.resources;
        if (!r) return;
        const [ux, uy] = worldPixel(lonLat, UNITS), unit = anchorAt(...lonLat).unit;
        this.herdSide = 2 * Math.ceil(HERD_REACH_M / (HERD_CELL * unit)) + 2;
        this.rockSide = 2 * Math.ceil(ROCK_REACH_M / (ROCK_CELL * unit)) + 2;
        const params = new ArrayBuffer(DRESSING_BYTES), ints = new Int32Array(params), words = new Uint32Array(params), floats = new Float32Array(params);
        ints.set([Math.floor(ux / HERD_CELL) - this.herdSide / 2, Math.floor(uy / HERD_CELL) - this.herdSide / 2]);
        words.set([this.herdSide, this.rockSide], 2);
        ints.set([Math.floor(ux / ROCK_CELL) - this.rockSide / 2, Math.floor(uy / ROCK_CELL) - this.rockSide / 2], 4);
        words[6] = this.rockModel ?? 0;
        floats[8] = HERD_REACH_M;
        if (this.show) {
            const [sx, sy] = worldPixel(this.show.lonLat, UNITS);
            ints.set([Math.round(sx), Math.round(sy)], 12);
            words.set([this.show.breeds.length, this.showFirst], 14);
        }
        r.gpu.device.queue.writeBuffer(r.params, 0, params);
    }

    encode(pass: GPUComputePassEncoder, ground: GPUBindGroup, view: number): void {
        const r = this.resources;
        if (!r || !this.surface || !this.features || !this.herdSide) return;
        pass.setBindGroup(0, r.groups[view]);
        pass.setBindGroup(1, this.surface);
        pass.setBindGroup(2, ground);
        pass.setBindGroup(3, this.features);
        pass.setPipeline(r.herds);
        pass.dispatchWorkgroups(this.herdSide, this.herdSide);
        if (this.rockModel === undefined) return;
        pass.setPipeline(r.rocks);
        pass.dispatchWorkgroups(Math.ceil(this.rockSide / 8), Math.ceil(this.rockSide / 8));
    }
}

/** The herds and the rocks about the eye. */
function dressingPipelines(gpu: Gpu): Promise<GPUComputePipeline[]> {
    const { device } = gpu, C = GPUShaderStage.COMPUTE, { surface, ground, features } = gpu.once(layouts);
    const group = device.createBindGroupLayout({ entries: [
        ...Instances.emitEntries(), { binding: 8, visibility: C, buffer: { type: 'uniform' } }, { binding: 9, visibility: C, buffer: { type: 'read-only-storage' } },
    ] });
    const module = device.createShaderModule({ label: 'dressing', code: wgsl('dressing/dressing.wgsl') });
    const layout = device.createPipelineLayout({ bindGroupLayouts: [group, surface, ground, features] });
    return Promise.all(['herds_of', 'rocks_of'].map(entryPoint => device.createComputePipelineAsync({ label: entryPoint, layout, compute: { module, entryPoint } })));
}
