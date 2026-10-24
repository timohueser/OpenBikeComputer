import { worldPixel } from '../../planner/layers/mercator';
import { anchorAt, UNITS } from '../frame/space';
import type { Gpu } from '../gpu/device';
import { wgsl } from '../gpu/wgsl';
import { COVER_ORDER } from '../ground/rules';
import { Instances, type ModelSpec, type Producer, type Surfaces } from '../instances/instances';
import { f16 } from '../instances/points';
import { TREES } from '../models/library';
import { PLANTS } from '../models/plant-library';
import { seasonOf, type Rules, type Season } from '../rules/regions';
import type { BuiltMesh, PlantJob } from './plant-worker';
import type { TreeModels } from './tree-models';

/** As plants.wgsl: the units of a cell of each grid, the reaches in metres, the list slots per region, vec4 per region and shapes per ground plant. */
const GROUND_UNITS = 32, SHRUB_UNITS = 256, BLADE_UNITS = 13, GROUND_M = 60, SHRUB_M = 500, BLADES_M = 30;
export const SLOTS = 16;
const REGION_WORDS = 1 + 2 * SLOTS, SEEDS = 3;
/** The plant that the blades stand for within BLADES_M. */
const SWARD = 'grass';
/** A tree model that stands as a shrub stands this tall; a plant model stands as its plant file says. */
const SHRUB_HEIGHT_M: [number, number] = [0.8, 1.8];
/** A plant model that stands as a shrub draws its mesh to this many metres, then its impostor. */
const SHRUB_MESH_M = 60;
/** How far a model takes its hue from the ground (draw.wgsl): a ground plant, and one that keeps its own colour or stands as a shrub, as the prototype's `tie`. */
const TIE = 0.85, OWN_TIE = 0.35;
const MOOR = ['heath', 'bog'], NONE = 0xffff;
const HABITAT = { moor: 1 << 12, lush: 1 << 13 }, CLUMPED = 1 << 14, MOOR_REGION = 1;

interface Resources { gpu: Gpu; ground: GPUComputePipeline; shrubs: GPUComputePipeline; blades: GPUComputePipeline; group: GPUBindGroup; params: GPUBuffer }

/** A plant model of the instances: a shape of a ground plant, a plant that stands as a shrub, or the patch of blades; in its season or evergreen. */
interface Model { model: string; seed: number; evergreen: boolean; shrub: boolean; tie: number; blades?: boolean }

/** The ground plants of a region that its list holds: the commonest SLOTS, the rarer first, so the common ones do not take every spot. */
export function groundList<T extends { share: number }>(plants: T[]): T[] {
    return [...plants].sort((a, b) => b.share - a.share).slice(0, SLOTS).sort((a, b) => a.share - b.share);
}

/**
 * The ground plants and the shrubs about the eye (plants.wgsl): a producer of instances whose models
 * come from the packs of the regions, built for the date's season. A tree that stands as a shrub is
 * a model of the forest's (tree-models.ts).
 */
export class Plants implements Producer {
    private resources?: Resources;
    private readonly list: Model[] = [];
    private readonly indexes: number[] = [];
    private readonly data: Uint32Array<ArrayBuffer>;
    private readonly worker = new Worker(new URL('./plant-worker.ts', import.meta.url), { type: 'module' });
    private season?: Season;
    private building = 0;
    private next = 0;
    private surface?: GPUBindGroup;
    private features?: GPUBindGroup;
    private sides: [number, number, number] = [0, 0, 0];
    /** The model of the patch of blades, and the first model of the plant that it stands for, when the near sward is blades. */
    private readonly sward?: [number, number];
    /** Whether the last update changed what the plants draw, other than by the camera. */
    newContent = false;

    /** With `blades`, a sward of blades stands within 30 m in place of the grass tufts. */
    constructor(rules: Rules, trees: TreeModels, private readonly instances: Instances, blades: boolean, private readonly wake: () => void) {
        instances.produce(this);
        const { materials } = rules.vocabulary;
        const data = new Uint32Array(64 + rules.regions.length * REGION_WORDS * 4).fill(NONE), floats = new Float32Array(data.buffer);
        materials.forEach(({ id, cover }, code) => (data[code] = Math.max(0, COVER_ORDER.indexOf(cover as (typeof COVER_ORDER)[number])) | (MOOR.includes(id) ? 256 : 0)));
        // The first of a run of models, one per entry.
        const models = (entries: Model[]) => {
            const at = this.list.findIndex(m => m.model === entries[0].model && m.shrub === entries[0].shrub && m.evergreen === entries[0].evergreen && m.tie === entries[0].tie && !!m.blades === !!entries[0].blades);
            if (at >= 0) return this.indexes[at];
            const first = instances.add(...entries.map(() => ({ tiers: [] })));
            if (first === undefined) return undefined;
            entries.forEach((entry, k) => (this.list.push(entry), this.indexes.push(first + k)));
            return first;
        };
        rules.regions.forEach((region, r) => {
            const at = 64 + r * REGION_WORDS * 4;
            data.fill(0, at, at + 4);
            data[at] = region.look.moor ? MOOR_REGION : 0;
            const ground = groundList(region.ground.filter(rule => rule.model in PLANTS).map(rule => ({ ...PLANTS[rule.model].ground, ...rule }))).flatMap(rule => {
                const tie = rule.own ? OWN_TIE : TIE;
                const first = models(Array.from({ length: SEEDS }, (_, k) => ({ model: rule.model, seed: k + 1, evergreen: false, shrub: false, tie })));
                return first === undefined ? [] : [{ ...rule, first }];
            });
            ground.forEach((rule, k) => {
                const on = rule.on.reduce((bits, cover) => bits | (1 << COVER_ORDER.indexOf(cover as (typeof COVER_ORDER)[number])), 0);
                const w = at + 4 * (1 + k);
                data.set([rule.first, on | (rule.habitat ? HABITAT[rule.habitat] : 0) | (rule.clumped ? CLUMPED : 0), 0, f16(rule.height[0]) | (f16(rule.height[1]) << 16)], w);
                floats[w + 2] = rule.share;
            });
            const shrubs = region.trees.filter(rule => rule.cover === 'shrub' && (rule.model in PLANTS || rule.model in TREES)).sort((a, b) => b.weight - a.weight).slice(0, SLOTS).flatMap(rule => {
                const plant = rule.model in PLANTS;
                const model = plant ? models([{ model: rule.model, seed: 1, evergreen: !!rule.evergreen, shrub: true, tie: OWN_TIE }]) : trees.index(rule.model, rule.evergreen, OWN_TIE);
                return model === undefined ? [] : [{ ...rule, index: model, height: plant ? PLANTS[rule.model].ground.height : SHRUB_HEIGHT_M }];
            });
            shrubs.forEach((rule, k) => {
                const w = at + 4 * (1 + SLOTS + k);
                data.set([rule.index, f16(rule.elevation[0]) | (f16(rule.elevation[1]) << 16), 0, f16(rule.height[0]) | (f16(rule.height[1]) << 16)], w);
                floats[w + 2] = rule.weight;
            });
        });
        this.data = data;
        const grass = this.list.findIndex(m => m.model === SWARD && !m.shrub);
        const patch = blades && grass >= 0 ? models([{ model: 'blades', seed: 1, evergreen: false, shrub: false, tie: TIE, blades: true }]) : undefined;
        if (patch !== undefined) this.sward = [patch, this.indexes[grass]];
        this.worker.onmessage = ({ data: message }: MessageEvent<{ id: number; built?: BuiltMesh[]; error?: string }>) => {
            this.building--;
            if (message.error || !message.built) return void console.warn(`Plant models: ${message.error}`);
            if (message.id !== this.next - 1) return;
            this.list.forEach((entry, i) => this.instances.replace(this.indexes[i], spec(entry, message.built![i])));
            this.newContent = true;
            this.wake();
        };
    }

    get idle(): boolean {
        return this.building === 0;
    }

    async attach(gpu: Gpu, frame: GPUBuffer, { surface, features, ground }: Surfaces): Promise<void> {
        this.resources = undefined;
        const { device } = gpu, C = GPUShaderStage.COMPUTE;
        const data = gpu.buffer('vegetation', this.data.byteLength, GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST);
        const params = gpu.buffer('vegetation', 64, GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST);
        device.queue.writeBuffer(data, 0, this.data);
        const layout = device.createBindGroupLayout({ entries: [
            ...Instances.emitEntries(), { binding: 8, visibility: C, buffer: { type: 'read-only-storage' } }, { binding: 9, visibility: C, buffer: { type: 'uniform' } },
        ] });
        const module = device.createShaderModule({ label: 'plants', code: wgsl('vegetation/plants.wgsl') });
        const pipelineLayout = device.createPipelineLayout({ bindGroupLayouts: [layout, surface.layout, ground, features.layout] });
        const [groundPlants, shrubs, blades] = await Promise.all(['ground_plants', 'shrubs', 'blades'].map(entryPoint => device.createComputePipelineAsync({ label: entryPoint, layout: pipelineLayout, compute: { module, entryPoint } })));
        this.resources = {
            gpu, ground: groundPlants, shrubs, blades, params,
            group: device.createBindGroup({ layout, entries: [...this.instances.emitResources(frame), { binding: 8, resource: { buffer: data } }, { binding: 9, resource: { buffer: params } }] }),
        };
        this.surface = surface.group;
        this.features = features.group;
    }

    /** The models of the date's season, and the windows of the grids about the eye at (lon, lat), `above` metres over the ground. */
    update(lonLat: [number, number], date: string, above: number): void {
        this.newContent = false;
        const r = this.resources;
        if (!r) return;
        const season = seasonOf(date);
        if (season !== this.season && this.list.length) {
            this.season = season;
            this.building++;
            this.worker.postMessage({ id: this.next++, models: this.list.map(({ model, seed, evergreen, blades }) => ({ model, seed, blades, season: evergreen ? 'summer' : season })) } satisfies PlantJob);
        }
        const [ux, uy] = worldPixel(lonLat, UNITS), unit = anchorAt(...lonLat).unit;
        // An eye higher over the ground than a grid's reach dispatches none of it.
        const window = (units: number, metres: number) => {
            const side = above < metres ? Math.ceil(2 * metres / (units * unit)) + 2 : 0;
            return [Math.floor(ux / units) - side / 2, Math.floor(uy / units) - side / 2].map(Math.floor).concat(side, 0);
        };
        const params = new Int32Array([...window(GROUND_UNITS, GROUND_M), ...window(SHRUB_UNITS, SHRUB_M), ...window(BLADE_UNITS, this.sward ? BLADES_M : 0), NONE, 0, 0, 0]);
        if (this.sward) params.set([this.sward[0]], 11), params.set([this.sward[1]], 12);
        new Float32Array(params.buffer)[13] = Math.max(0, above);
        this.sides = [params[2], params[6], params[10]];
        r.gpu.device.queue.writeBuffer(r.params, 0, params);
    }

    encode(pass: GPUComputePassEncoder, ground: GPUBindGroup): void {
        const r = this.resources;
        if (!r || !this.surface || !this.features) return;
        pass.setBindGroup(0, r.group);
        pass.setBindGroup(1, this.surface);
        pass.setBindGroup(2, ground);
        pass.setBindGroup(3, this.features);
        const run = (pipeline: GPUComputePipeline, side: number) => {
            if (!side) return;
            pass.setPipeline(pipeline);
            pass.dispatchWorkgroups(Math.ceil(side / 8), Math.ceil(side / 8));
        };
        run(r.ground, this.sides[0]);
        run(r.shrubs, this.sides[1]);
        run(r.blades, this.sides[2]);
    }
}

/** The tiers of a plant model. */
function spec({ shrub, blades, tie }: Model, mesh: BuiltMesh): ModelSpec {
    if (shrub) return { tiers: [{ mesh, until: SHRUB_MESH_M }], impostor: { from: 0, until: SHRUB_M }, tie };
    return { tiers: [{ mesh, until: blades ? BLADES_M : GROUND_M }], tie };
}
