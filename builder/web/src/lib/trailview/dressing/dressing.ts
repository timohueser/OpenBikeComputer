import { worldPixel } from '../../planner/layers/mercator';
import { anchorAt, UNITS } from '../frame/space';
import type { Gpu } from '../gpu/device';
import { wgsl } from '../gpu/wgsl';
import { Instances, type Producer } from '../instances/instances';
import { animal, BREEDS } from '../models/animals';
import { geometry, type Model } from '../models/flat';
import { rock } from '../models/rocks';
import type { Rules } from '../rules/regions';

/** Herds stand within this many metres, rocks within this many; as HERDS and SLABS of dressing.wgsl. */
const HERD_REACH_M = 1500, ROCK_REACH_M = 400, HERDS = 4, SLABS = 3;
/** Units along a herd cell and a rock point's cell, as HERD_SHIFT and ROCK_SHIFT of dressing.wgsl. */
const HERD_CELL = 2 ** 14, ROCK_CELL = 2 ** 7;
/** Words of `Herd` and bytes of `Dressing` in dressing.wgsl. */
const HERD_WORDS = 8, DRESSING_BYTES = 48;

interface Resources { gpu: Gpu; herds: GPUComputePipeline; rocks: GPUComputePipeline; group: GPUBindGroup; params: GPUBuffer }

/**
 * The dressing of the ground (dressing.wgsl): the herds of the regions' packs on the materials that
 * they graze, and rocks on open and rocky ground, placed on the GPU each frame.
 */
export class Dressing implements Producer {
    private resources?: Resources;
    private readonly table: Uint32Array<ArrayBuffer>;
    private readonly rockModel: number;
    private herdSide = 0;
    private rockSide = 0;

    constructor(rules: Rules, private readonly instances: Instances) {
        instances.produce(this);
        const add = (model: Model, until: number) => instances.add({ tiers: [{ mesh: geometry(model), until }] });
        const rocks = (['slab', 'boulder'] as const).flatMap(name => Array.from({ length: SLABS }, (_, k) => add(rock(name, k + 1), ROCK_REACH_M)));
        this.rockModel = rocks[0];
        const breeds = new Map<string, number>(), used = new Set(rules.regions.flatMap(region => region.herds.map(herd => herd.model)).filter(name => name in BREEDS));
        for (const name of used) {
            breeds.set(name, BREEDS[name].coats.map((_, coat) => add(animal(name, coat), HERD_REACH_M))[0]);
        }
        const materials = rules.vocabulary.materials.map(material => material.id);
        this.table = new Uint32Array(Math.max(1, rules.regions.length) * HERDS * HERD_WORDS);
        const floats = new Float32Array(this.table.buffer);
        rules.regions.forEach((region, r) => {
            region.herds.filter(herd => breeds.has(herd.model)).slice(0, HERDS).forEach((herd, k) => {
                const at = (r * HERDS + k) * HERD_WORDS, grazes = [0, 0];
                for (const id of herd.on) {
                    const code = materials.indexOf(id);
                    if (code >= 0 && code < 64) grazes[code >> 5] |= 1 << (code & 31);
                }
                this.table.set([breeds.get(herd.model)!, BREEDS[herd.model].coats.length], at);
                floats.set([herd.groups, BREEDS[herd.model].height, ...herd.size], at + 2);
                this.table.set(grazes.map(bits => bits >>> 0), at + 6);
            });
        });
    }

    async attach(gpu: Gpu, frame: GPUBuffer, surface: { layout: GPUBindGroupLayout; group: GPUBindGroup }, features: { layout: GPUBindGroupLayout; group: GPUBindGroup }, ground: GPUBindGroupLayout): Promise<void> {
        this.resources = undefined;
        const { device } = gpu, C = GPUShaderStage.COMPUTE;
        const herds = gpu.buffer('dressing', this.table.byteLength, GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST);
        const params = gpu.buffer('dressing', DRESSING_BYTES, GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST);
        device.queue.writeBuffer(herds, 0, this.table);
        const layout = device.createBindGroupLayout({ entries: [
            ...Instances.emitEntries(), { binding: 8, visibility: C, buffer: { type: 'uniform' } }, { binding: 9, visibility: C, buffer: { type: 'read-only-storage' } },
        ] });
        const module = device.createShaderModule({ label: 'dressing', code: wgsl('dressing/dressing.wgsl') });
        const pipelineLayout = device.createPipelineLayout({ bindGroupLayouts: [layout, surface.layout, ground, features.layout] });
        const [herdsOf, rocksOf] = await Promise.all(['herds_of', 'rocks_of'].map(entryPoint => device.createComputePipelineAsync({ label: entryPoint, layout: pipelineLayout, compute: { module, entryPoint } })));
        this.resources = {
            gpu, herds: herdsOf, rocks: rocksOf, params,
            group: device.createBindGroup({ layout, entries: [...this.instances.emitResources(frame), { binding: 8, resource: { buffer: params } }, { binding: 9, resource: { buffer: herds } }] }),
        };
        this.surface = surface.group;
        this.features = features.group;
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
        words[6] = this.rockModel;
        floats.set([HERD_REACH_M, ROCK_REACH_M], 8);
        r.gpu.device.queue.writeBuffer(r.params, 0, params);
    }

    encode(pass: GPUComputePassEncoder, ground: GPUBindGroup): void {
        const r = this.resources;
        if (!r || !this.surface || !this.features || !this.herdSide) return;
        pass.setBindGroup(0, r.group);
        pass.setBindGroup(1, this.surface);
        pass.setBindGroup(2, ground);
        pass.setBindGroup(3, this.features);
        pass.setPipeline(r.herds);
        pass.dispatchWorkgroups(this.herdSide, this.herdSide);
        pass.setPipeline(r.rocks);
        pass.dispatchWorkgroups(Math.ceil(this.rockSide / 8), Math.ceil(this.rockSide / 8));
    }
}
