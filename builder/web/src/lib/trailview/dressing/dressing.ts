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
/** An animal draws its near tier to this many metres (specs/trailview-models.md). */
const ANIMAL_NEAR_M = 150;
/** Units along a herd cell and a rock point's cell, as HERD_SHIFT of dressing.wgsl and ROCK_SHIFT of rocks.wgsl. */
const HERD_CELL = 2 ** 14, ROCK_CELL = 2 ** 7;
/** Words of `Herd` and bytes of `Dressing` in dressing.wgsl. */
const HERD_WORDS = 8, DRESSING_BYTES = 64;

/**
 * The showroom (`showroom=lat,lon,heading,entries[,tier]` in the address), the engine view of the model review: in place
 * of the herds, a row of animals across the view toward `heading` (degrees from north) at (lat, lon), SHOW_SPACING
 * times their heights apart, each facing right and turned SHOW_TURN toward the eye. An entry is a breed, which stands all its models, or
 * `breed.k`, its model k. With `tier` 1 every animal draws its far tier at any distance.
 */
interface Showroom { lonLat: [number, number]; heading: number; entries: string[]; tier: number }
const SHOW_SPACING = 2.3, SHOW_TURN = 0.45;

function showroom(): Showroom | undefined {
    const value = globalThis.location ? new URLSearchParams(globalThis.location.search).get('showroom') : null;
    if (!value) return undefined;
    const [lat, lon, heading, entries, tier] = value.split(',');
    return { lonLat: [Number(lon), Number(lat)], heading: (Number(heading) * Math.PI) / 180, entries: entries.split('|'), tier: Number(tier ?? 0) };
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
    private readonly show = showroom();
    /** The first entry of the showroom's animals in the table, and their count. */
    private showFirst = 0;
    private showCount = 0;
    /** The files of the models. */
    private readonly urls: string[] = [];

    constructor(rules: Rules, private readonly instances: Instances, private readonly files: ModelFiles) {
        instances.produce(this);
        const { objects, breeds: breedModels } = files.models;
        const spec = ({ tiers: [mesh], shape }: Packed, until: number): ModelSpec => ({ tiers: [{ mesh, until }], shape });
        // No fade of the rocks' tier: rocks_of ends each rock at its own distance before the reach.
        this.rockModel = this.load(objects.file, objects.rocks, model => ({ ...spec(model, ROCK_REACH_M), fade: 0 }));
        const animal = ({ tiers: [near, far], shape }: Packed): ModelSpec => ({ tiers: this.show?.tier ? [{ mesh: far, until: HERD_REACH_M }] : [{ mesh: near, until: ANIMAL_NEAR_M }, { mesh: far, until: HERD_REACH_M }], shape });
        const shown = this.show?.entries.map(entry => entry.split('.')) ?? [];
        const breeds = new Map<string, number>(), used = new Set([...rules.regions.flatMap(region => region.herds.map(herd => herd.model)), ...shown.map(([name]) => name)].filter(name => name in breedModels));
        for (const name of used) {
            const first = this.load(breedModels[name].file, breedModels[name].coats * breedModels[name].poses, animal);
            if (first !== undefined) breeds.set(name, first);
        }
        // The showroom's animals: an entry of a breed stands each of its models.
        const row = shown.filter(([name]) => breeds.has(name)).flatMap(([name, k]) => (k ? [Number(k)] : Array.from({ length: breedModels[name].coats * breedModels[name].poses }, (_, m) => m)).map(m => ({ name, model: breeds.get(name)! + m })));
        this.showFirst = Math.max(1, rules.regions.length) * HERDS;
        this.showCount = row.length;
        this.table = new Uint32Array((this.showFirst + row.length) * HERD_WORDS);
        const floats = new Float32Array(this.table.buffer);
        if (this.show) {
            const across = -this.show.heading, room = row.map(({ name }) => SHOW_SPACING * breedModels[name].height);
            let offset = -room.reduce((sum, r) => sum + r, 0) / 2;
            row.forEach(({ name, model }, k) => {
                const at = (this.showFirst + k) * HERD_WORDS;
                offset += room[k] / 2;
                this.table[at] = model;
                floats.set([across - Math.PI / 2 - SHOW_TURN, breedModels[name].height, offset * Math.cos(across), offset * Math.sin(across)], at + 2);
                offset += room[k] / 2;
            });
        }
        rules.regions.forEach((region, r) => {
            region.herds.filter(herd => breeds.has(herd.model)).slice(0, HERDS).forEach((herd, k) => {
                const at = (r * HERDS + k) * HERD_WORDS, { coats, poses } = breedModels[herd.model];
                this.table.set([breeds.get(herd.model)!, coats | (poses << 16)], at);
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
            words.set([this.showCount, this.showFirst], 14);
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
