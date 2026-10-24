import type { PlannerConfig } from '../../planner/config';
import { worldPixel } from '../../planner/layers/mercator';
import { anchorAt, UNITS } from '../frame/space';
import type { Gpu } from '../gpu/device';
import { wgsl } from '../gpu/wgsl';
import { Instances, type ModelSpec, type Producer, type Surfaces } from '../instances/instances';
import { SEASONS, type Rules, type Season } from '../rules/regions';
import { AROUND, Canopy, CANOPY_ZOOM } from './canopy';
import type { BuiltModel } from './model-worker';

/** Trees stand within this many metres of the eye: the stamps of main roads reach 4 km. */
const REACH_M = 4500;
/** Past this many metres the trees thin with the distance; cells of 2^k × 2^k base cells start at 2^k times it. */
const THIN_M = 600;
/** The mesh tiers of a tree: the prototype's three levels of detail, and where each ends; impostors follow to the reach. */
const TIERS_M = [30, 150, 700];
/** Units along a cell edge, as CELL in forest.wgsl, and blue-noise points per cell. */
const CELL = 2048, POINTS = 512;
/** Tree species per region, as SPECIES in forest.wgsl and tools/trailview/rules.py. */
const SPECIES = 32;
/** Trees per hectare of each stems class: the middle of its range, and 700 for 600 or more (specs/trailview-tiles.md#canopy). */
const STEMS_HA = [25, 75, 125, 175, 250, 350, 500, 700];
const FOREST_BYTES = 64;
/** Where `Data` of forest.wgsl holds the points and the species; the canopy window comes first. */
const POINTS_AT = 272, SPECIES_AT = POINTS_AT + 8 * POINTS;
/** Units along a canopy tile's edge (zoom CANOPY_ZOOM). */
const TILE = UNITS / 2 ** CANOPY_ZOOM;

interface Resources {
    gpu: Gpu;
    findCells: GPUComputePipeline;
    trees: GPUComputePipeline;
    /** Group 0 of `find_cells`, which writes the cells, and of `trees`, which reads them and dispatches by their count. */
    groups: [GPUBindGroup, GPUBindGroup];
    cells: GPUBuffer;
    params: GPUBuffer;
    surface: GPUBindGroup;
    features: GPUBindGroup;
}

/**
 * The forest: the canopy layer about the eye, the tree models of the season, and the producer of
 * instances (forest.wgsl) that places the trees each frame.
 */
export class Forest implements Producer {
    private resources?: Resources;
    private readonly canopy: Canopy;
    /** The models by `model|evergreen`, as instance model indexes. */
    private readonly models = new Map<string, number>();
    private readonly worker = new Worker(new URL('./model-worker.ts', import.meta.url), { type: 'module' });
    private season?: Season;
    private building = 0;
    private next = 0;
    /** Whether models arrived since the last update. */
    private rebuilt = false;
    private species = new Float32Array(0);
    private window: [number, number] = [0, 0];
    private side = 0;
    /** Base cells a side of the window at the highest latitude that the view serves: the cell list's size. */
    private readonly most: number;
    /** Metres from the eye where trees stand, at the instances' detail 1. */
    readonly reach = REACH_M;
    /** Whether the last update changed what the forest draws, other than by the camera. */
    newContent = false;

    constructor(config: PlannerConfig, rules: Rules, private readonly instances: Instances, private readonly wake: () => void) {
        instances.produce(this);
        this.canopy = new Canopy(config, wake);
        this.most = windowSide(Math.max(Math.abs(config.bounds[1]), Math.abs(config.bounds[3])));
        const words = new Uint32Array(rules.regions.length * SPECIES * 4), table = new Float32Array(words.buffer);
        rules.regions.forEach((region, r) => {
            // tools/trailview/rules.py checks that a region has SPECIES tree species at most, each with its model.
            region.trees.filter(rule => rule.cover === 'tree').slice(0, SPECIES).forEach((rule, k) => {
                const name = `${rule.model}|${rule.evergreen ? 1 : 0}`;
                if (!this.models.has(name)) this.models.set(name, instances.add({ tiers: [] }));
                words[4 * (r * SPECIES + k)] = this.models.get(name)! | ((rule.leaf === 'broadleaved' ? 1 : 2) << 16);
                table.set([...rule.elevation, rule.weight], 4 * (r * SPECIES + k) + 1);
            });
        });
        this.species = table;
        this.worker.onmessage = ({ data }: MessageEvent<{ id: number; built?: BuiltModel[]; error?: string }>) => {
            this.building--;
            if (data.error || !data.built) return void console.warn(`Tree models: ${data.error}`);
            if (data.id !== this.next - 1) return;
            [...this.models.values()].forEach((model, i) => this.instances.replace(model, this.spec(data.built![i])));
            this.rebuilt = true;
            this.wake();
        };
    }

    get idle(): boolean {
        return this.canopy.idle && this.building === 0;
    }

    get requests(): { requests: number; bytes: number; failed: number } {
        const { requests, bytes, failed } = this.canopy;
        return { requests, bytes, failed };
    }

    async attach(gpu: Gpu, frame: GPUBuffer, { surface, features, ground }: Surfaces): Promise<void> {
        this.resources = undefined;
        this.canopy.lost();
        const { device } = gpu, C = GPUShaderStage.COMPUTE;
        const storage = GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST;
        const data = gpu.buffer('vegetation', SPECIES_AT + Math.max(16, this.species.byteLength), storage);
        const cells = gpu.buffer('vegetation', 16 + 16 * this.most * this.most, storage | GPUBufferUsage.INDIRECT);
        const params = gpu.buffer('vegetation', FOREST_BYTES, GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST);
        const texture = this.canopy.attach(gpu, data);
        device.queue.writeBuffer(data, POINTS_AT, bluePoints(POINTS));
        device.queue.writeBuffer(data, SPECIES_AT, this.species);
        const shared: GPUBindGroupLayoutEntry[] = [
            ...Instances.emitEntries(), { binding: 8, visibility: C, texture: { sampleType: 'uint', viewDimension: '2d-array' } },
            { binding: 9, visibility: C, buffer: { type: 'read-only-storage' } }, { binding: 11, visibility: C, buffer: { type: 'uniform' } },
        ];
        const layouts = [
            device.createBindGroupLayout({ entries: [...shared, { binding: 10, visibility: C, buffer: { type: 'storage' } }] }),
            device.createBindGroupLayout({ entries: [...shared, { binding: 12, visibility: C, buffer: { type: 'read-only-storage' } }] }),
        ];
        const module = device.createShaderModule({ label: 'forest', code: wgsl('vegetation/forest.wgsl') });
        const [findCells, trees] = await Promise.all(['find_cells', 'trees'].map((entryPoint, k) => device.createComputePipelineAsync({
            label: entryPoint, layout: device.createPipelineLayout({ bindGroupLayouts: [layouts[k], surface.layout, ground, features.layout] }), compute: { module, entryPoint },
        })));
        const entries = [
            ...this.instances.emitResources(frame), { binding: 8, resource: texture.createView({ dimension: '2d-array' }) },
            { binding: 9, resource: { buffer: data } }, { binding: 11, resource: { buffer: params } },
        ];
        this.resources = {
            gpu, findCells, trees, cells, params, surface: surface.group, features: features.group,
            groups: [
                device.createBindGroup({ layout: layouts[0], entries: [...entries, { binding: 10, resource: { buffer: cells } }] }),
                device.createBindGroup({ layout: layouts[1], entries: [...entries, { binding: 12, resource: { buffer: cells } }] }),
            ],
        };
    }

    /** Takes the canopy tiles about the eye at (lon, lat), and the models of the date's season. */
    update(lonLat: [number, number], date: string): void {
        this.newContent = this.rebuilt;
        this.rebuilt = false;
        const r = this.resources;
        if (!r) return;
        const [x, y] = worldPixel(lonLat, 2 ** CANOPY_ZOOM), { unit } = anchorAt(...lonLat);
        this.canopy.update(x, y, Math.min(AROUND, Math.ceil(this.reach / (TILE * unit))));
        this.newContent ||= this.canopy.changed;
        const season = SEASONS[Math.floor((Number(date.slice(5, 7)) - 1) / 3)];
        if (season !== this.season) {
            this.season = season;
            this.building++;
            this.worker.postMessage({ id: this.next++, models: [...this.models.keys()].map(name => name.split('|')).map(([model, evergreen]) => ({ model, season: evergreen === '1' ? 'summer' : season })) });
        }
        const [ux, uy] = worldPixel(lonLat, UNITS);
        this.side = Math.min(this.most, windowSide(lonLat[1]));
        // Whole cells of the coarsest level, as LEVELS of forest.wgsl.
        this.window = [ux, uy].map(u => 8 * Math.floor((u / CELL - this.side / 2) / 8)) as [number, number];
        const params = new ArrayBuffer(FOREST_BYTES), ints = new Int32Array(params), floats = new Float32Array(params);
        ints.set(this.window);
        new Uint32Array(params).set([this.side, POINTS], 2);
        floats.set([this.reach, THIN_M], 4);
        floats.set(STEMS_HA.map(n => n / 1e4), 8);
        r.gpu.device.queue.writeBuffer(r.params, 0, params);
        r.gpu.device.queue.writeBuffer(r.cells, 0, new Uint32Array([0, 1, 1, 0]));
    }

    /** Encodes the placement into a compute pass, with the ground pages in group 2. */
    encode(pass: GPUComputePassEncoder, ground: GPUBindGroup): void {
        const r = this.resources;
        if (!r) return;
        pass.setBindGroup(0, r.groups[0]);
        pass.setBindGroup(1, r.surface);
        pass.setBindGroup(2, ground);
        pass.setBindGroup(3, r.features);
        pass.setPipeline(r.findCells);
        pass.dispatchWorkgroups(Math.ceil(this.side / 8), Math.ceil(this.side / 8));
        pass.setPipeline(r.trees);
        pass.setBindGroup(0, r.groups[1]);
        pass.dispatchWorkgroupsIndirect(r.cells, 0);
    }

    /** A tree model's tiers. */
    private spec({ tiers, crown }: BuiltModel): ModelSpec {
        return { tiers: tiers.map((mesh, t) => ({ mesh, until: TIERS_M[t] })), impostor: { from: 1, until: this.reach }, crown };
    }
}

/** Base cells a side of the window that holds the reach at a latitude, in whole cells of the coarsest level and their margin. */
function windowSide(lat: number): number {
    return 8 * Math.ceil((2 * REACH_M / (CELL * anchorAt(0, lat).unit) + 16) / 8);
}

/**
 * Progressive blue noise in the unit square, which tiles: each point is the one of a few candidates
 * farthest from the points before it, across the square's edges. Every first n points spread evenly.
 */
export function bluePoints(n: number, candidates = 24): Float32Array<ArrayBuffer> {
    let seed = 0x2545f491;
    const random = () => {
        seed ^= seed << 13;
        seed ^= seed >>> 17;
        seed ^= seed << 5;
        return (seed >>> 0) / 2 ** 32;
    };
    const out = new Float32Array(2 * n);
    for (let i = 0; i < n; i++) {
        let best = -1;
        for (let c = 0; c < (i ? candidates : 1); c++) {
            const x = random(), y = random();
            let near = Infinity;
            for (let j = 0; j < i; j++) {
                const dx = Math.abs(x - out[2 * j]), dy = Math.abs(y - out[2 * j + 1]);
                near = Math.min(near, Math.min(dx, 1 - dx) ** 2 + Math.min(dy, 1 - dy) ** 2);
            }
            if (near > best) {
                best = near;
                out.set([x, y], 2 * i);
            }
        }
    }
    return out;
}
