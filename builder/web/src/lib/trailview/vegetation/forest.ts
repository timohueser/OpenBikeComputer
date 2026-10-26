import type { PlannerConfig } from '../../planner/config';
import { worldPixel } from '../../planner/layers/mercator';
import { anchorAt, UNITS } from '../frame/space';
import { size, type Node } from '../terrain/lod';
import type { Gpu } from '../gpu/device';
import { layouts } from '../gpu/layouts';
import { wgsl } from '../gpu/wgsl';
import { Instances, VIEWS, type Producer, type Surfaces } from '../instances/instances';
import type { Rules } from '../rules/regions';
import { AROUND, Canopy, CANOPY_ZOOM } from './canopy';
import { TREE_REACH_M, type TreeModels } from './tree-models';

/** Past this many metres the trees thin with the distance; cells of 2^k × 2^k base cells start at 2^k times it. */
const THIN_M = 600;
/** The share of the canopy's trees that stand: the owner's choice by eye. A lower share takes the last points of each cell and moves no other tree. */
const DENSITY = 0.5;
/** Units along a cell edge, as CELL in forest.wgsl, and blue-noise points per cell. */
const CELL = 2048, POINTS = 512;
/** Tree species per region, as tools/trailview/rules.py checks. */
const SPECIES = 32;
/** Trees per hectare of each stems class: the middle of its range, and 700 for 600 or more (specs/trailview-tiles.md#canopy). */
const STEMS_HA = [25, 75, 125, 175, 250, 350, 500, 700];
const FOREST_BYTES = 96;
/** Where `Data` of forest.wgsl holds the points and the species; the canopy window comes first. */
const POINTS_AT = 272, SPECIES_AT = POINTS_AT + 8 * POINTS;
/** Units along a canopy tile's edge (zoom CANOPY_ZOOM). */
const TILE = UNITS / 2 ** CANOPY_ZOOM;
/** The level of the ground pages whose regions the trees take, as REGION_LEVEL of forest.wgsl. */
const REGION_LEVEL = 10;

interface Resources {
    gpu: Gpu;
    findCells: GPUComputePipeline;
    trees: GPUComputePipeline;
    /** Per view the group 0 of `find_cells`, which writes its cells, and of `trees`, which reads them and dispatches by their count. */
    groups: [GPUBindGroup, GPUBindGroup][];
    cells: GPUBuffer[];
    params: GPUBuffer;
    surface: GPUBindGroup;
    features: GPUBindGroup;
}

/**
 * The forest: the canopy layer about the eye and the producer of instances (forest.wgsl) that places
 * the trees of the season's models (tree-models.ts) each frame.
 */
export class Forest implements Producer {
    private resources?: Resources;
    private readonly canopy: Canopy;
    private readonly species: Uint32Array<ArrayBuffer>;
    /** The material code of orchards, or 0xffffffff for none. */
    private readonly orchard: number;
    private window: [number, number] = [0, 0];
    private side = 0;
    /** Base cells a side of the window at the highest latitude that the view serves: the cell list's size. */
    private readonly most: number;
    /** Metres from the eye where trees stand, at the instances' detail 1. */
    readonly reach = TREE_REACH_M;
    /** Whether the last update changed what the forest draws, other than by the camera. */
    newContent = false;

    constructor(config: PlannerConfig, rules: Rules, trees: TreeModels, private readonly instances: Instances, wake: () => void) {
        instances.produce(this);
        this.canopy = new Canopy(config, wake);
        this.most = windowSide(Math.max(Math.abs(config.bounds[1]), Math.abs(config.bounds[3])));
        this.orchard = rules.vocabulary.materials.findIndex(material => material.id === 'orchard') >>> 0;
        // tools/trailview/rules.py checks that a region has SPECIES tree species at most, each with its model.
        this.species = speciesTable(rules.regions.map(region => region.trees.filter(rule => rule.cover === 'tree').slice(0, SPECIES).flatMap(rule => {
            const model = trees.index(rule.model, rule.evergreen);
            return model === undefined ? [] : [{ model, leaf: rule.leaf === 'broadleaved' ? 1 : 2, band: rule.elevation, weight: rule.weight }];
        })));
    }

    get idle(): boolean {
        return this.canopy.idle;
    }

    get requests(): { requests: number; bytes: number; failed: number } {
        const { requests, bytes, failed } = this.canopy;
        return { requests, bytes, failed };
    }

    async attach(gpu: Gpu, { surface, features }: Surfaces): Promise<void> {
        this.resources = undefined;
        this.canopy.lost();
        const { device } = gpu;
        const storage = GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST;
        const data = gpu.buffer('vegetation', SPECIES_AT + Math.max(16, this.species.byteLength), storage);
        const cells = VIEWS.map(() => gpu.buffer('vegetation', 32 + 16 * listParts(this.most).at(-1)!, storage | GPUBufferUsage.INDIRECT));
        const params = gpu.buffer('vegetation', FOREST_BYTES, GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST);
        const texture = this.canopy.attach(gpu, data);
        device.queue.writeBuffer(data, POINTS_AT, bluePoints(POINTS));
        device.queue.writeBuffer(data, SPECIES_AT, this.species);
        const [findCells, trees] = await gpu.once(forestPipelines);
        this.resources = {
            gpu, findCells, trees, cells, params, surface, features,
            groups: VIEWS.map(view => {
                const entries = [
                    ...this.instances.emitResources(view), { binding: 8, resource: texture.createView({ dimension: '2d-array' }) },
                    { binding: 9, resource: { buffer: data } }, { binding: 11, resource: { buffer: params } },
                ];
                return [
                    device.createBindGroup({ layout: findCells.getBindGroupLayout(0), entries: [...entries, { binding: 10, resource: { buffer: cells[view] } }] }),
                    device.createBindGroup({ layout: trees.getBindGroupLayout(0), entries: [...entries, { binding: 12, resource: { buffer: cells[view] } }] }),
                ];
            }),
        };
    }

    /**
     * The ground pages of REGION_LEVEL within the reach of the eye at (lon, lat): a tree takes its
     * species from them, also where the terrain draws finer pages, so they come with the first pages.
     */
    regionPages(lonLat: [number, number]): Node[] {
        const [ux, uy] = worldPixel(lonLat, UNITS), reach = this.reach / anchorAt(...lonLat).unit, side = size(REGION_LEVEL), nodes: Node[] = [];
        for (let y = Math.floor((uy - reach) / side); y <= Math.floor((uy + reach) / side); y++) {
            for (let x = Math.floor((ux - reach) / side); x <= Math.floor((ux + reach) / side); x++) nodes.push({ level: REGION_LEVEL, x, y });
        }
        const far = ({ x, y }: Node) => Math.hypot((x + 0.5) * side - ux, (y + 0.5) * side - uy);
        return nodes.sort((a, b) => far(a) - far(b));
    }

    /** Takes the canopy tiles about the eye at (lon, lat). */
    update(lonLat: [number, number]): void {
        this.newContent = false;
        const r = this.resources;
        if (!r) return;
        const [x, y] = worldPixel(lonLat, 2 ** CANOPY_ZOOM), { unit } = anchorAt(...lonLat);
        this.canopy.update(x, y, Math.min(AROUND, Math.ceil(this.reach / (TILE * unit))));
        this.newContent = this.canopy.changed;
        const [ux, uy] = worldPixel(lonLat, UNITS);
        this.side = Math.min(this.most, windowSide(lonLat[1]));
        // Whole cells of the coarsest level, as LEVELS of forest.wgsl.
        this.window = [ux, uy].map(u => 8 * Math.floor((u / CELL - this.side / 2) / 8)) as [number, number];
        const params = new ArrayBuffer(FOREST_BYTES), ints = new Int32Array(params), floats = new Float32Array(params);
        ints.set(this.window);
        new Uint32Array(params).set([this.side, POINTS], 2);
        floats.set([this.reach, THIN_M], 4);
        new Uint32Array(params)[6] = this.orchard;
        floats[7] = DENSITY;
        floats.set(STEMS_HA.map(n => n / 1e4), 8);
        new Uint32Array(params).set(listParts(this.most), 16);
        r.gpu.device.queue.writeBuffer(r.params, 0, params);
        for (const cells of r.cells) r.gpu.device.queue.writeBuffer(cells, 0, new Uint32Array([0, 1, 1, 0, 0, 0, 0, 0]));
    }

    /** Encodes the placement into a compute pass, with the ground pages in group 2. */
    encode(pass: GPUComputePassEncoder, ground: GPUBindGroup, view: number): void {
        const r = this.resources;
        if (!r) return;
        pass.setBindGroup(0, r.groups[view][0]);
        pass.setBindGroup(1, r.surface);
        pass.setBindGroup(2, ground);
        pass.setBindGroup(3, r.features);
        pass.setPipeline(r.findCells);
        pass.dispatchWorkgroups(Math.ceil(this.side / 8), Math.ceil(this.side / 8));
        pass.setPipeline(r.trees);
        pass.setBindGroup(0, r.groups[view][1]);
        pass.dispatchWorkgroupsIndirect(r.cells[view], 0);
    }
}

/** A tree species of a region: its model, leaf type (1 broadleaved, 2 needleleaved), elevation band in metres and weight. */
export interface Species { model: number; leaf: number; band: [number, number]; weight: number }

/**
 * The species choices of the regions, as `species_of` of forest.wgsl reads them, in u32 words: the
 * regions, then per region where its block starts. A block holds its n band edges (f32), then per slot
 * of the elevation (2i between edge i - 1 and edge i, 2i + 1 on edge i) and leaf type (0 any) the
 * start, count and total weight of its list, then the lists of (model, weight). A list holds the
 * species of the first rule that has any: in their band with the leaf type, in their band, with the
 * leaf type, all. The sums are f32 in the order of the species, as a scan of them on the GPU would add.
 */
export function speciesTable(regions: Species[][]): Uint32Array<ArrayBuffer> {
    const words: number[] = [regions.length, ...regions.map(() => 0)], f32 = new Float32Array(1), u32 = new Uint32Array(f32.buffer);
    const bits = (value: number) => ((f32[0] = value), u32[0]);
    regions.forEach((all, r) => {
        // A weight of 0 ends a region's list.
        const ended = all.findIndex(s => !(Math.fround(s.weight) > 0)), species = ended < 0 ? all : all.slice(0, ended);
        const edges = [...new Set(species.flatMap(({ band }) => band.map(Math.fround)))].sort((a, b) => a - b), n = edges.length;
        const at = (words[1 + r] = words.length);
        words.push(n, ...edges.map(bits));
        const slots = at + 1 + n, lists: number[] = [];
        words.push(...new Array(3 * 3 * (2 * n + 1)).fill(0));
        for (let slot = 0; slot <= 2 * n; slot++) {
            const i = slot >> 1, e = slot % 2 ? edges[i] : i === 0 ? (edges[0] ?? 0) - 1 : i === n ? edges[n - 1] + 1 : (edges[i - 1] + edges[i]) / 2;
            for (let leaf = 0; leaf < 3; leaf++) {
                const fits = species.map(s => [e >= Math.fround(s.band[0]) && e <= Math.fround(s.band[1]), leaf === 0 || s.leaf === leaf]);
                const rules = [([band, same]: boolean[]) => band && same, ([band]: boolean[]) => band, ([, same]: boolean[]) => same, () => true];
                const totals = rules.map(rule => species.reduce((sum, s, k) => (rule(fits[k]) ? Math.fround(sum + Math.fround(s.weight)) : sum), 0));
                const rule = Math.max(0, totals.findIndex(total => total > 0));
                const list = species.filter((_, k) => rules[rule](fits[k]));
                words.splice(slots + 3 * (3 * slot + leaf), 3, words.length + lists.length, list.length, bits(totals[rule]));
                lists.push(...list.flatMap(s => [s.model, bits(s.weight)]));
            }
        }
        words.push(...lists);
    });
    return Uint32Array.from(words);
}

/**
 * Where the cells of each level start in the cell list (forest.wgsl), and its entries: room for every
 * cell of a level in a window of `side` base cells.
 */
function listParts(side: number): number[] {
    const parts = [0];
    for (let level = 0; level <= 3; level++) parts.push(parts[level] + ((side >> level) + 1) ** 2);
    return parts;
}

/** Base cells a side of the window that holds the reach at a latitude, in whole cells of the coarsest level and their margin. */
function windowSide(lat: number): number {
    return 8 * Math.ceil((2 * TREE_REACH_M / (CELL * anchorAt(0, lat).unit) + 16) / 8);
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

/** The cells of the trees about the eye, and the trees in them. */
function forestPipelines(gpu: Gpu): Promise<GPUComputePipeline[]> {
    const { device } = gpu, C = GPUShaderStage.COMPUTE, { surface, ground, features } = gpu.once(layouts);
    const shared: GPUBindGroupLayoutEntry[] = [
        ...Instances.emitEntries(), { binding: 8, visibility: C, texture: { sampleType: 'uint', viewDimension: '2d-array' } },
        { binding: 9, visibility: C, buffer: { type: 'read-only-storage' } }, { binding: 11, visibility: C, buffer: { type: 'uniform' } },
    ];
    const groups = [
        device.createBindGroupLayout({ entries: [...shared, { binding: 10, visibility: C, buffer: { type: 'storage' } }] }),
        device.createBindGroupLayout({ entries: [...shared, { binding: 12, visibility: C, buffer: { type: 'read-only-storage' } }] }),
    ];
    const module = device.createShaderModule({ label: 'forest', code: wgsl('vegetation/forest.wgsl') });
    return Promise.all(['find_cells', 'trees'].map((entryPoint, k) => device.createComputePipelineAsync({
        label: entryPoint, layout: device.createPipelineLayout({ bindGroupLayouts: [groups[k], surface, ground, features] }), compute: { module, entryPoint },
    })));
}
