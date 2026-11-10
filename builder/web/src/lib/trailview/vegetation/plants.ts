import { worldPixel } from '../../planner/layers/mercator';
import { anchorAt, UNITS } from '../frame/space';
import type { Gpu } from '../gpu/device';
import { layouts } from '../gpu/layouts';
import { wgsl } from '../gpu/wgsl';
import { COVER_ORDER } from '../ground/rules';
import { EYE, Instances, VIEWS, type ModelSpec, type Producer, type Surfaces } from '../instances/instances';
import { f16 } from '../instances/points';
import { TREES } from '../models/library';
import { PLANTS } from '../models/plant-library';
import { bits, named, seasonOf, type Region, type Rules, type Season, type Vocabulary } from '../rules/regions';
import type { BuiltMesh, PlantJob } from './plant-worker';
import type { Reach, TreeModels } from './tree-models';

/** As plants.wgsl: the units of a cell of each grid, the reaches in metres, the list slots per region, vec4 per region and shapes per ground plant. */
const GROUND_UNITS = 32, SHRUB_UNITS = 256, GROUND_M = 60, SHRUB_M = 500;
export const SLOTS = 16;
const REGION_WORDS = 3 * SLOTS;
/** Shapes per ground plant and per plant that stands in rows, consecutive models. */
export const SEEDS = 3;
/** A tree model that stands as a shrub stands this tall; a plant model stands as its plant file says. */
const SHRUB_HEIGHT_M: [number, number] = [0.8, 1.8];
/** A plant model that stands as a shrub draws its mesh to this many metres, then its impostor. */
const SHRUB_MESH_M = 60;
/** How far a model takes its hue from the ground (draw.wgsl): a ground plant, and one that keeps its own colour or stands as a shrub, as the prototype's `tie`. */
const TIE = 0.85, OWN_TIE = 0.35;
const NONE = 0xffff;
export const CLUMPED = 1;

/** A material's word of plants.wgsl: its cover, and its share of its cover's shrubs in 255ths at bit 16. */
export function materialWord({ cover, shrubs = 1 }: Vocabulary['materials'][number]): number {
    return Math.max(0, COVER_ORDER.indexOf(cover as (typeof COVER_ORDER)[number])) | (Math.round(shrubs * 255) << 16);
}

/**
 * The ground plants of a region that its list holds, what the pack says of each over its plant
 * model's `ground`, with their words of plants.wgsl: CLUMPED and at bit 16 the salt of its stands, and
 * the bits of the materials that it grows on. The salt comes from the species, so its stands stay put
 * whatever the shares of the other plants.
 */
export function groundRules(region: Region, vocabulary: Vocabulary) {
    return groundList(region.ground.filter(rule => rule.model in PLANTS).map(rule => ({ ...PLANTS[rule.model].ground, ...rule }))).map(rule => {
        const salt = [...rule.id].reduce((h, c) => (31 * h + c.charCodeAt(0)) % 997, 7), off = named(vocabulary, rule.off);
        return { ...rule, word: (rule.clumped ? CLUMPED : 0) | (salt << 16), materials: bits(named(vocabulary, rule.on).filter(code => !off.includes(code))) };
    });
}

interface Resources { gpu: Gpu; ground: GPUComputePipeline; shrubs: GPUComputePipeline; groups: GPUBindGroup[]; params: GPUBuffer }

/** A plant model of the instances: a shape of a ground plant or a plant that stands as a shrub; in its season or evergreen. */
interface Model { model: string; seed: number; evergreen: boolean; shrub: boolean; tie: number }

/** The ground plants of a region that its list holds: the commonest SLOTS, the rarer first, so the common ones do not take every spot. */
export function groundList<T extends { share: number }>(plants: T[]): T[] {
    return [...plants].sort((a, b) => b.share - a.share).slice(0, SLOTS).sort((a, b) => a.share - b.share);
}

/**
 * The ground plants and the shrubs about the eye (plants.wgsl): a producer of instances whose models
 * come from the packs of the regions, built for the date's season once their region is in reach. A tree that stands as a shrub is
 * a model of the forest's (tree-models.ts).
 */
export class Plants implements Producer {
    private resources?: Resources;
    private readonly list: Model[] = [];
    private readonly indexes: number[] = [];
    /** Per model of the list: the regions that plant it, and the seasons that its mesh was asked for and that it holds. */
    private readonly users: Set<number>[] = [];
    private readonly asked: (Season | undefined)[] = [];
    private readonly built: (Season | undefined)[] = [];
    private readonly data: Uint32Array<ArrayBuffer>;
    private readonly worker = new Worker(new URL('./plant-worker.ts', import.meta.url), { type: 'module' });
    /** The jobs in the worker by id: their season and their models of the list. */
    private readonly jobs = new Map<number, { season: Season; models: number[] }>();
    private season?: Season;
    private reach: Reach = { reach: new Set(), near: new Set() };
    private next = 0;
    private surface?: GPUBindGroup;
    private features?: GPUBindGroup;
    private sides: [number, number] = [0, 0];
    /** Whether the last update changed what the plants draw, other than by the camera. */
    newContent = false;

    /** `regions` gives the regions in reach. An abort of `stop` ends the worker. */
    constructor(rules: Rules, trees: TreeModels, private readonly instances: Instances, private readonly wake: () => void, private readonly regions: () => Reach, stop: AbortSignal) {
        stop.addEventListener('abort', () => this.worker.terminate());
        instances.produce(this);
        const { materials } = rules.vocabulary;
        const data = new Uint32Array(64 + rules.regions.length * REGION_WORDS * 4).fill(NONE), floats = new Float32Array(data.buffer);
        materials.forEach((material, code) => (data[code] = materialWord(material)));
        rules.regions.forEach((region, r) => {
            const at = 64 + r * REGION_WORDS * 4;
            const ground = groundRules(region, rules.vocabulary).flatMap(rule => {
                const tie = rule.own ? OWN_TIE : TIE;
                const first = this.models(r, Array.from({ length: SEEDS }, (_, k) => ({ model: rule.model, seed: k + 1, evergreen: false, shrub: false, tie })));
                return first === undefined ? [] : [{ ...rule, first }];
            });
            ground.forEach((rule, k) => {
                const w = at + 8 * k;
                data.set([rule.first, rule.word, 0, f16(rule.height[0]) | (f16(rule.height[1]) << 16)], w);
                floats[w + 2] = rule.share;
                data.set([...rule.materials, 0, 0], w + 4);
            });
            const shrubs = region.trees.filter(rule => rule.cover === 'shrub' && (rule.model in PLANTS || rule.model in TREES)).sort((a, b) => b.weight - a.weight).slice(0, SLOTS).flatMap(rule => {
                const plant = rule.model in PLANTS;
                const model = plant ? this.models(r, [{ model: rule.model, seed: 1, evergreen: !!rule.evergreen, shrub: true, tie: OWN_TIE }]) : trees.index(r, rule.model, rule.evergreen, OWN_TIE);
                return model === undefined ? [] : [{ ...rule, index: model, height: plant ? PLANTS[rule.model].ground.height : SHRUB_HEIGHT_M }];
            });
            shrubs.forEach((rule, k) => {
                const w = at + 4 * (2 * SLOTS + k);
                data.set([rule.index, f16(rule.elevation[0]) | (f16(rule.elevation[1]) << 16), 0, f16(rule.height[0]) | (f16(rule.height[1]) << 16)], w);
                floats[w + 2] = rule.weight;
            });
        });
        this.data = data;
        this.worker.onmessage = ({ data: message }: MessageEvent<{ id: number; built?: BuiltMesh[]; error?: string }>) => {
            const job = this.jobs.get(message.id)!;
            this.jobs.delete(message.id);
            if (message.error || !message.built) return void console.warn(`Plant models: ${message.error}`);
            if (job.season !== this.season) return;
            job.models.forEach((i, k) => {
                this.instances.replace(this.indexes[i], spec(this.list[i], message.built![k]));
                this.built[i] = job.season;
            });
            this.newContent = true;
            this.wake();
        };
    }

    get idle(): boolean {
        return this.jobs.size === 0 && this.list.every((_, i) => this.asked[i] === this.season || !this.plants(i, this.reach.reach));
    }

    /** Whether the models of the near regions hold their meshes of the season. */
    get nearIdle(): boolean {
        return this.list.every((_, i) => this.built[i] === this.season || !this.plants(i, this.reach.near));
    }

    /** The first of SEEDS models of a plant that stands in rows (rows.ts) of region `region`, in its own colour; ask before the first update. */
    rowModel(region: number, model: string): number | undefined {
        return this.models(region, Array.from({ length: SEEDS }, (_, k) => ({ model, seed: k + 1, evergreen: false, shrub: true, tie: OWN_TIE })));
    }

    /** Whether one of these regions plants model `i` of the list. */
    private plants(i: number, regions: Set<number>): boolean {
        return [...this.users[i]].some(region => regions.has(region));
    }

    /** The first of a run of models of region `region`, one per entry: a run that the list already holds whole, else a new one. */
    private models(region: number, entries: Model[]): number | undefined {
        const same = (m: Model | undefined, e: Model) => m?.model === e.model && m.seed === e.seed && m.shrub === e.shrub && m.evergreen === e.evergreen && m.tie === e.tie;
        let at = this.list.findIndex((_, i) => entries.every((entry, k) => same(this.list[i + k], entry) && this.indexes[i + k] === this.indexes[i] + k));
        if (at < 0) {
            const first = this.instances.add(...entries.map(() => ({ tiers: [] })));
            if (first === undefined) return undefined;
            at = this.list.length;
            entries.forEach((entry, k) => (this.list.push(entry), this.indexes.push(first + k), this.users.push(new Set())));
        }
        entries.forEach((_, k) => this.users[at + k].add(region));
        return this.indexes[at];
    }

    async attach(gpu: Gpu, { surface, features }: Surfaces): Promise<void> {
        this.resources = undefined;
        const { device } = gpu;
        const data = gpu.buffer('vegetation', this.data.byteLength, GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST);
        const params = gpu.buffer('vegetation', 64, GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST);
        device.queue.writeBuffer(data, 0, this.data);
        const [groundPlants, shrubs] = await gpu.once(plantPipelines);
        this.resources = {
            gpu, ground: groundPlants, shrubs, params,
            groups: VIEWS.map(view => device.createBindGroup({ layout: groundPlants.getBindGroupLayout(0), entries: [...this.instances.emitResources(view), { binding: 8, resource: { buffer: data } }, { binding: 9, resource: { buffer: params } }] })),
        };
        this.surface = surface;
        this.features = features;
    }

    /** The models of the date's season, and the windows of the grids about the eye at (lon, lat), `above` metres over the ground. */
    update(lonLat: [number, number], date: string, above: number): void {
        this.newContent = false;
        const r = this.resources;
        if (!r) return;
        const season = (this.season = seasonOf(date)), reach = (this.reach = this.regions());
        // A job for the near regions' models first, then one for the others in reach, each in the season.
        for (const regions of [reach.near, reach.reach]) {
            const models = this.list.flatMap((_, i) => (this.asked[i] !== season && this.plants(i, regions) ? [i] : []));
            if (!models.length) continue;
            models.forEach(i => (this.asked[i] = season));
            const id = this.next++;
            this.jobs.set(id, { season, models });
            this.worker.postMessage({ id, models: models.map(i => ({ model: this.list[i].model, seed: this.list[i].seed, season: this.list[i].evergreen ? 'summer' : season })) } satisfies PlantJob);
        }
        const [ux, uy] = worldPixel(lonLat, UNITS), unit = anchorAt(...lonLat).unit;
        // An eye higher over the ground than a grid's reach dispatches none of it.
        const window = (units: number, metres: number) => {
            const side = above < metres ? Math.ceil(2 * metres / (units * unit)) + 2 : 0;
            return [Math.floor(ux / units) - side / 2, Math.floor(uy / units) - side / 2].map(Math.floor).concat(side, 0);
        };
        const params = new Int32Array([...window(GROUND_UNITS, GROUND_M), ...window(SHRUB_UNITS, SHRUB_M), 0, 0, 0, 0]);
        new Float32Array(params.buffer)[8] = Math.max(0, above);
        this.sides = [params[2], params[6]];
        r.gpu.device.queue.writeBuffer(r.params, 0, params);
    }

    encode(pass: GPUComputePassEncoder, ground: GPUBindGroup, view: number): void {
        const r = this.resources;
        if (!r || !this.surface || !this.features) return;
        pass.setBindGroup(0, r.groups[view]);
        pass.setBindGroup(1, this.surface);
        pass.setBindGroup(2, ground);
        pass.setBindGroup(3, this.features);
        const run = (pipeline: GPUComputePipeline, side: number) => {
            if (!side) return;
            pass.setPipeline(pipeline);
            pass.dispatchWorkgroups(Math.ceil(side / 8), Math.ceil(side / 8));
        };
        // The ground plants cast no shadow.
        if (view === EYE) run(r.ground, this.sides[0]);
        run(r.shrubs, this.sides[1]);
    }
}

/** The tiers of a plant model. */
function spec({ shrub, tie }: Model, mesh: BuiltMesh): ModelSpec {
    if (shrub) return { tiers: [{ mesh, until: SHRUB_MESH_M }], impostor: { from: 0, until: SHRUB_M }, tie };
    return { tiers: [{ mesh, until: GROUND_M }], tie };
}

/** The ground plants and the shrubs about the eye. */
function plantPipelines(gpu: Gpu): Promise<GPUComputePipeline[]> {
    const { device } = gpu, C = GPUShaderStage.COMPUTE, { surface, ground, features } = gpu.once(layouts);
    const group = device.createBindGroupLayout({ entries: [
        ...Instances.emitEntries(), { binding: 8, visibility: C, buffer: { type: 'read-only-storage' } }, { binding: 9, visibility: C, buffer: { type: 'uniform' } },
    ] });
    const module = device.createShaderModule({ label: 'plants', code: wgsl('vegetation/plants.wgsl') });
    const layout = device.createPipelineLayout({ bindGroupLayouts: [group, surface, ground, features] });
    return Promise.all(['ground_plants', 'shrubs'].map(entryPoint => device.createComputePipelineAsync({ label: entryPoint, layout, compute: { module, entryPoint } })));
}
