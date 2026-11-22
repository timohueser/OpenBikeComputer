import { worldPixel } from '../../planner/layers/mercator';
import { anchorAt, UNITS } from '../frame/space';
import type { Gpu } from '../gpu/device';
import { layouts } from '../gpu/layouts';
import { wgsl } from '../gpu/wgsl';
import { Instances, VIEWS, type ModelSpec, type Producer, type Surfaces } from '../instances/instances';
import type { ModelFiles } from '../instances/model-files';
import type { Packed } from '../models/model-file';
import { bits, named, SEASONS, type Rules } from '../rules/regions';
import type { Reach } from '../vegetation/tree-models';

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
 * of the herds, a row of models across the view toward `heading` (degrees from north) at (lat, lon), each facing right
 * and turned SHOW_TURN toward the eye. An entry is a breed, which stands all its models, or `breed.k`, its model k, an
 * animal SHOW_SPACING times its height wide; with `tier` 1 every animal draws its far tier at any distance. An entry
 * `tree.<id>.<tier>[.<room>]` stands that tier of a tree species, SHOW_TREE_M tall, in `room` metres of the row (SHOW_TREE_M
 * by default), in the crown of the date's phase.
 * A showroom stands on the ground but does not read its colour, so its trees take no satellite hue: they show their own colours.
 */
interface Showroom { lonLat: [number, number]; heading: number; entries: string[]; tier: number }
const SHOW_SPACING = 2.3, SHOW_TURN = 0.45, SHOW_TREE_M = 10;

function showroom(): Showroom | undefined {
    const value = globalThis.location ? new URLSearchParams(globalThis.location.search).get('showroom') : null;
    if (!value) return undefined;
    const [lat, lon, heading, entries, tier] = value.split(',');
    if (!entries) return undefined;
    return { lonLat: [Number(lon), Number(lat)], heading: (Number(heading) * Math.PI) / 180, entries: entries.split('|'), tier: Number(tier ?? 0) };
}

interface Resources { gpu: Gpu; herds: GPUComputePipeline; rocks: GPUComputePipeline; groups: GPUBindGroup[]; params: GPUBuffer; table: GPUBuffer }

/**
 * The dressing of the ground (dressing.wgsl): the herds of the regions' packs on the materials that
 * they graze, and rocks on rough and rocky ground, placed on the GPU each frame. Their models come from
 * model files (models/model-file.ts): the rocks from the objects file, each breed from a file of its own. A
 * region's herds take their models when the region comes into reach: all breeds of all packs would not fit
 * in the instances' models.
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
    /** The first instance model of each breed that has its models, and the regions whose herds are in the table. */
    private readonly breeds = new Map<string, number | undefined>();
    private readonly placed = new Set<number>();
    private changed = false;
    private updated = false;

    constructor(private readonly rules: Rules, private readonly instances: Instances, private readonly files: ModelFiles) {
        instances.produce(this);
        const { objects, breeds: breedModels } = files.models;
        const spec = ({ tiers: [mesh], shape }: Packed, until: number): ModelSpec => ({ tiers: [{ mesh, until }], shape });
        // No fade of the rocks' tier: rocks_of ends each rock at its own distance before the reach.
        this.rockModel = this.load(objects.file, objects.rocks, model => ({ ...spec(model, ROCK_REACH_M), fade: 0 }));
        // The showroom's models: an entry of a breed stands each of its models.
        const row = (this.show?.entries ?? []).map(entry => entry.split('.')).flatMap(([name, k, tier, room]) => {
            const first = name === 'tree' ? this.tree(k, Number(tier ?? 0)) : this.breed(name);
            if (first === undefined) return [];
            if (name === 'tree') return [{ model: first, height: SHOW_TREE_M, room: Number(room) || SHOW_TREE_M }];
            const { coats, poses, height } = breedModels[name];
            return (k ? [Number(k)] : Array.from({ length: coats * poses }, (_, m) => m)).map(m => ({ model: first + m, height, room: SHOW_SPACING * height }));
        });
        this.showFirst = Math.max(1, rules.regions.length) * HERDS;
        this.showCount = row.length;
        this.table = new Uint32Array((this.showFirst + row.length) * HERD_WORDS);
        const floats = new Float32Array(this.table.buffer);
        if (this.show) {
            const across = -this.show.heading;
            let offset = -row.reduce((sum, { room }) => sum + room, 0) / 2;
            row.forEach(({ model, height, room }, k) => {
                const at = (this.showFirst + k) * HERD_WORDS;
                offset += room / 2;
                this.table[at] = model;
                floats.set([across - Math.PI / 2 - SHOW_TURN, height, offset * Math.cos(across), offset * Math.sin(across)], at + 2);
                offset += room / 2;
            });
        }
    }

    /** The first instance model of a breed, which it adds on the first ask; undefined where the instances have no room. */
    private breed(name: string): number | undefined {
        const entry = this.files.models.breeds[name];
        if (!entry) return undefined;
        if (!this.breeds.has(name)) {
            const near = this.show?.tier ? [] : [ANIMAL_NEAR_M];
            this.breeds.set(name, this.load(entry.file, entry.coats * entry.poses, ({ tiers, shape }) => ({ tiers: [...near, HERD_REACH_M].map((until, t) => ({ mesh: tiers[t + 1 - near.length], until })), shape })));
        }
        return this.breeds.get(name);
    }

    /**
     * The instance model of a showroom tree's tier: its summer crown, whose phases draw the crown of a phase's own or the
     * summer crown through the phase's colour matrix, as the forest's trees do (vegetation/tree-models.ts).
     */
    private tree(id: string, tier: number): number | undefined {
        const entry = this.files.models.trees[id];
        if (!entry) return undefined;
        const spec = ({ tiers, shape }: Packed): ModelSpec => ({ tiers: [{ mesh: tiers[tier], until: HERD_REACH_M }], shape });
        const own = entry.crowns.map(file => (file ? this.load(file, 1, spec) : undefined));
        const summer: number | undefined = this.load(entry.summer, 1, model => ({
            ...spec(model), phases: SEASONS.map((_, phase) => (own[phase] === undefined ? { model: summer!, tone: entry.tones[phase] ?? undefined } : { model: own[phase]! })),
        }));
        return summer;
    }

    /** Writes the herds of region `r` into the table, in the order of its pack: those whose breed has its models. */
    private place(r: number): void {
        this.placed.add(r);
        this.changed = true;
        const floats = new Float32Array(this.table.buffer), { breeds } = this.files.models;
        // Only the first HERDS herds with a model draw, so only theirs take instance models.
        this.rules.regions[r].herds.filter(herd => herd.model in breeds).slice(0, HERDS).filter(herd => this.breed(herd.model) !== undefined).forEach((herd, k) => {
            const at = (r * HERDS + k) * HERD_WORDS, { coats, poses, height } = breeds[herd.model];
            this.table.set([this.breed(herd.model)!, coats | (poses << 16)], at);
            floats.set([herd.groups, height, ...herd.size], at + 2);
            this.table.set(bits(named(this.rules.vocabulary, herd.on)), at + 6);
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

    /** Whether the models of the regions in reach have arrived, or failed. */
    get idle(): boolean {
        return this.updated && this.files.done(...this.urls);
    }

    async attach(gpu: Gpu, { surface, features }: Surfaces): Promise<void> {
        this.resources = undefined;
        const { device } = gpu;
        const herds = gpu.buffer('dressing', this.table.byteLength, GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST);
        const params = gpu.buffer('dressing', DRESSING_BYTES, GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST);
        this.changed = true;
        const [herdsOf, rocksOf] = await gpu.once(dressingPipelines);
        this.resources = {
            gpu, herds: herdsOf, rocks: rocksOf, params, table: herds,
            groups: VIEWS.map(view => device.createBindGroup({ layout: herdsOf.getBindGroupLayout(0), entries: [...this.instances.emitResources(view), { binding: 8, resource: { buffer: params } }, { binding: 9, resource: { buffer: herds } }] })),
        };
        this.surface = surface;
        this.features = features;
    }

    private surface?: GPUBindGroup;
    private features?: GPUBindGroup;

    /** The windows of herd cells and rock points about the eye at (lon, lat), and the herds of the regions in reach. */
    update(lonLat: [number, number], { reach }: Reach): void {
        const r = this.resources;
        if (!r) return;
        // A showroom stands in place of the herds: they take no models.
        if (!this.showCount) for (const region of reach) if (!this.placed.has(region) && this.rules.regions[region]) this.place(region);
        if (this.changed) r.gpu.device.queue.writeBuffer(r.table, 0, this.table);
        this.changed = false;
        this.updated = true;
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
