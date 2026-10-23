import { worldPixel } from '../../planner/layers/mercator';
import { anchorAt, UNITS } from '../frame/space';
import type { Gpu } from '../gpu/device';
import { wgsl } from '../gpu/wgsl';
import { COVER_ORDER } from '../ground/rules';
import { Instances, type ModelSpec, type Producer } from '../instances/instances';
import { TREES } from '../models/library';
import { PLANTS } from '../models/plant-library';
import { SEASONS, type Rules, type Season } from '../rules/regions';
import type { BuiltMesh } from './plant-worker';

/** As plants.wgsl: the units of a cell of each grid, the radii in metres, the list slots per region and vec4 per region. */
const GROUND_UNITS = 32, SHRUB_UNITS = 256, GROUND_M = 60, SHRUB_M = 500, SLOTS = 8, REGION_WORDS = 17;
/** A tree model that stands as a shrub draws its middle level to this many metres, then its coarsest. */
const SHRUB_MESH_M = 150;
/** A tree model that stands as a shrub stands this tall; a plant model stands as its plant file says. */
const SHRUB_HEIGHT_M: [number, number] = [0.8, 1.8];
const MOOR = ['heath', 'bog'], NONE = 0xffff;
const HABITAT = { moor: 1 << 12, lush: 1 << 13 }, CLUMPED = 1 << 14, MOOR_REGION = 1;

interface Resources { gpu: Gpu; ground: GPUComputePipeline; shrubs: GPUComputePipeline; group: GPUBindGroup; params: GPUBuffer }

/** A model of the plants: a plant or a tree, as a ground plant or a shrub, in its season or evergreen. */
interface Model { model: string; tree: boolean; shrub: boolean; evergreen: boolean }

const f16 = (a: number, b: number) => {
    const view = new DataView(new ArrayBuffer(4)), half = (x: number) => {
        view.setFloat32(0, x);
        const bits = view.getUint32(0), e = ((bits >> 23) & 255) - 112;
        return e <= 0 ? 0 : Math.min(0x7bff, (e << 10) + ((bits >> 13) & 0x3ff) + ((bits >> 12) & 1));
    };
    return half(a) | (half(b) << 16);
};

/**
 * The ground plants and the shrubs about the eye (plants.wgsl): a producer of instances whose models
 * come from the packs of the regions, built for the date's season.
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
    private sides: [number, number] = [0, 0];
    /** Whether the last update changed what the plants draw, other than by the camera. */
    newContent = false;

    constructor(rules: Rules, private readonly instances: Instances, private readonly wake: () => void) {
        instances.produce(this);
        const { materials } = rules.vocabulary;
        const data = new Uint32Array(64 + rules.regions.length * REGION_WORDS * 4).fill(NONE), floats = new Float32Array(data.buffer);
        materials.forEach(({ id, cover }, code) => (data[code] = Math.max(0, COVER_ORDER.indexOf(cover as (typeof COVER_ORDER)[number])) | (MOOR.includes(id) ? 256 : 0)));
        const model = (entry: Model) => {
            const at = this.list.findIndex(m => m.model === entry.model && m.shrub === entry.shrub && m.evergreen === entry.evergreen);
            if (at >= 0) return this.indexes[at];
            this.list.push(entry);
            this.indexes.push(instances.add({ tiers: [] }));
            return this.indexes[this.indexes.length - 1];
        };
        rules.regions.forEach((region, r) => {
            const at = 64 + r * REGION_WORDS * 4;
            data.fill(0, at, at + 4);
            data[at] = region.look.moor ? MOOR_REGION : 0;
            // The rarer plants first, so the common one does not take every spot.
            const ground = region.ground.filter(rule => rule.model in PLANTS).map(rule => ({ ...PLANTS[rule.model].ground, ...rule }))
                .sort((a, b) => a.share - b.share).slice(0, SLOTS);
            ground.forEach((rule, k) => {
                const on = rule.on.reduce((bits, cover) => bits | (1 << COVER_ORDER.indexOf(cover as (typeof COVER_ORDER)[number])), 0);
                const w = at + 4 * (1 + k);
                data.set([model({ model: rule.model, tree: false, shrub: false, evergreen: false }),
                    on | (rule.habitat ? HABITAT[rule.habitat] : 0) | (rule.clumped ? CLUMPED : 0), 0, f16(...rule.height)], w);
                floats[w + 2] = rule.share;
            });
            const shrubs = region.trees.filter(rule => rule.cover === 'shrub' && (rule.model in PLANTS || rule.model in TREES)).slice(0, SLOTS);
            shrubs.forEach((rule, k) => {
                const tree = !(rule.model in PLANTS), w = at + 4 * (1 + SLOTS + k);
                data.set([model({ model: rule.model, tree, shrub: true, evergreen: !!rule.evergreen }), f16(...rule.elevation), 0,
                    f16(...(tree ? SHRUB_HEIGHT_M : PLANTS[rule.model].ground.height))], w);
                floats[w + 2] = rule.weight;
            });
        });
        this.data = data;
        this.worker.onmessage = ({ data: message }: MessageEvent<{ id: number; built?: BuiltMesh[][]; error?: string }>) => {
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

    async attach(gpu: Gpu, frame: GPUBuffer, surface: { layout: GPUBindGroupLayout; group: GPUBindGroup }, features: { layout: GPUBindGroupLayout; group: GPUBindGroup }, ground: GPUBindGroupLayout): Promise<void> {
        this.resources = undefined;
        this.season = undefined;
        const { device } = gpu, C = GPUShaderStage.COMPUTE;
        const data = gpu.buffer('vegetation', this.data.byteLength, GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST);
        const params = gpu.buffer('vegetation', 32, GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST);
        device.queue.writeBuffer(data, 0, this.data);
        const layout = device.createBindGroupLayout({ entries: [
            ...Instances.emitEntries(), { binding: 8, visibility: C, buffer: { type: 'read-only-storage' } }, { binding: 9, visibility: C, buffer: { type: 'uniform' } },
        ] });
        const module = device.createShaderModule({ label: 'plants', code: wgsl('vegetation/plants.wgsl') });
        const pipelineLayout = device.createPipelineLayout({ bindGroupLayouts: [layout, surface.layout, ground, features.layout] });
        const [groundPlants, shrubs] = await Promise.all(['ground_plants', 'shrubs'].map(entryPoint => device.createComputePipelineAsync({ label: entryPoint, layout: pipelineLayout, compute: { module, entryPoint } })));
        this.resources = {
            gpu, ground: groundPlants, shrubs, params,
            group: device.createBindGroup({ layout, entries: [...this.instances.emitResources(frame), { binding: 8, resource: { buffer: data } }, { binding: 9, resource: { buffer: params } }] }),
        };
        this.surface = surface.group;
        this.features = features.group;
    }

    /** The models of the date's season, and the windows of the grids about the eye at (lon, lat). */
    update(lonLat: [number, number], date: string): void {
        this.newContent = false;
        const r = this.resources;
        if (!r) return;
        const season = SEASONS[Math.floor((Number(date.slice(5, 7)) - 1) / 3)];
        if (season !== this.season) {
            this.season = season;
            this.building++;
            this.worker.postMessage({ id: this.next++, models: this.list.map(({ model, tree, evergreen }) => ({ model, tree, season: evergreen ? 'summer' : season })) });
        }
        const [ux, uy] = worldPixel(lonLat, UNITS), unit = anchorAt(...lonLat).unit;
        const window = (units: number, metres: number) => {
            const side = Math.ceil(2 * metres / (units * unit)) + 2;
            return [Math.floor(ux / units) - side / 2, Math.floor(uy / units) - side / 2].map(Math.floor).concat(side, 0);
        };
        const params = new Int32Array([...window(GROUND_UNITS, GROUND_M), ...window(SHRUB_UNITS, SHRUB_M)]);
        this.sides = [params[2], params[6]];
        r.gpu.device.queue.writeBuffer(r.params, 0, params);
    }

    encode(pass: GPUComputePassEncoder, ground: GPUBindGroup): void {
        const r = this.resources;
        if (!r || !this.surface || !this.features) return;
        pass.setBindGroup(0, r.group);
        pass.setBindGroup(1, this.surface);
        pass.setBindGroup(2, ground);
        pass.setBindGroup(3, this.features);
        pass.setPipeline(r.ground);
        pass.dispatchWorkgroups(Math.ceil(this.sides[0] / 8), Math.ceil(this.sides[0] / 8));
        pass.setPipeline(r.shrubs);
        pass.dispatchWorkgroups(Math.ceil(this.sides[1] / 8), Math.ceil(this.sides[1] / 8));
    }
}

/** The tiers of a model of the plants. */
function spec({ tree, shrub }: Model, meshes: BuiltMesh[]): ModelSpec {
    if (tree) return { tiers: [{ mesh: meshes[0], until: SHRUB_MESH_M }, { mesh: meshes[1], until: SHRUB_M }] };
    return { tiers: [{ mesh: meshes[0], until: shrub ? SHRUB_M : GROUND_M }] };
}
