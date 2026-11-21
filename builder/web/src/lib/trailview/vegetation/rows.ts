import type { PlannerConfig } from '../../planner/config';
import { worldPixel } from '../../planner/layers/mercator';
import { anchorAt, UNITS } from '../frame/space';
import type { Gpu } from '../gpu/device';
import { layouts } from '../gpu/layouts';
import { wgsl } from '../gpu/wgsl';
import { Instances, VIEWS, type Producer, type Surfaces } from '../instances/instances';
import { TREES } from '../models/library';
import { PLANTS } from '../models/plant-library';
import type { Region, Rules } from '../rules/regions';
import { SEEDS, type Plants } from './plants';
import type { TreeModels } from './tree-models';

/** As rows.wgsl: the crops at most, the units along a base cell, and the bits of a model word. */
const CROPS = 4, BASE = 2 ** 11, NO_CROP = 0xffffffff, NO_MODEL = 0xffff, WINDWARD = 1 << 16, ALONG = 1 << 17, SHAPES = 18;
/** Words of `Data` in rows.wgsl before its models: the crop of each material code, then the crops. */
const MODELS_AT = 64 + 4 * CROPS;

/** Bytes of the header of `RowCells` in rows.wgsl and of a listed cell. */
const HEADER_BYTES = 16, CELL_BYTES = 16;

interface Resources {
    gpu: Gpu;
    findRows: GPUComputePipeline;
    plantings: GPUComputePipeline;
    /** Per view the group 0 of `find_rows`, which lists the cells in view, and of `plantings`, which reads them and dispatches by their count. */
    groups: [GPUBindGroup, GPUBindGroup][];
    cells: GPUBuffer[];
    params: GPUBuffer;
    surface: GPUBindGroup;
    features: GPUBindGroup;
}

/**
 * The plantings (rows.wgsl): the plants of the materials whose `rows` (materials.json) say how they are
 * planted, on the rows of the material fields, placed on the GPU each frame. A region plants the model
 * that its pack's `rows` names for a material, else the material's own. A tree is a model of the
 * forest's (tree-models.ts), a plant one of the plants' (plants.ts).
 */
export class Rows implements Producer {
    private resources?: Resources;
    private readonly data: Uint32Array<ArrayBuffer>;
    private readonly reach: number;
    /** Base cells a side of the window at the highest latitude that the view serves: the cell list's size. */
    private readonly most: number;
    private side = 0;

    constructor(config: PlannerConfig, rules: Rules, trees: TreeModels, plants: Plants, private readonly instances: Instances) {
        instances.produce(this);
        // tools/trailview/rules.py checks that CROPS materials at most have rows.
        const crops = rules.vocabulary.materials.flatMap(({ rows }, code) => (rows ? [{ code, rows }] : [])).slice(0, CROPS);
        const data = new Uint32Array(MODELS_AT + 4 * Math.max(1, rules.regions.length)).fill(NO_CROP), floats = new Float32Array(data.buffer);
        crops.forEach(({ code, rows }, c) => {
            data[code] = c;
            data[64 + 4 * c] = code;
            floats.set([rows.reach_m, ...rows.height_m], 64 + 4 * c + 1);
        });
        const evergreen = (region: Region, model: string) => (region.trees.find(rule => rule.model === model) ?? rules.regions.flatMap(r => r.trees).find(rule => rule.model === model))?.evergreen;
        const word = (region: Region, r: number, model: string | undefined) => {
            if (model && model in PLANTS) {
                const first = plants.rowModel(r, model);
                return first === undefined ? NO_MODEL : first | ALONG | (SEEDS << SHAPES);
            }
            const index = model && model in TREES ? trees.index(r, model, evergreen(region, model)) : undefined;
            return index === undefined ? NO_MODEL : index | (TREES[model!].keepsHeading ? WINDWARD : 0);
        };
        rules.regions.forEach((region, r) => crops.forEach(({ code, rows }, c) => {
            data[MODELS_AT + 4 * r + c] = word(region, r, region.rows[rules.vocabulary.materials[code].id] ?? rows.model);
        }));
        this.data = data;
        this.reach = Math.max(0, ...crops.map(({ rows }) => rows.reach_m));
        this.most = this.sideAt(Math.max(Math.abs(config.bounds[1]), Math.abs(config.bounds[3])));
    }

    async attach(gpu: Gpu, { surface, features }: Surfaces): Promise<void> {
        this.resources = undefined;
        const { device } = gpu;
        const data = gpu.buffer('vegetation', this.data.byteLength, GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST);
        const params = gpu.buffer('vegetation', 16, GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST);
        device.queue.writeBuffer(data, 0, this.data);
        const cells = VIEWS.map(() => gpu.buffer('vegetation', HEADER_BYTES + CELL_BYTES * Math.max(1, this.most ** 2), GPUBufferUsage.STORAGE | GPUBufferUsage.INDIRECT | GPUBufferUsage.COPY_DST));
        const [findRows, plantings] = await gpu.once(rowsPipelines);
        this.resources = {
            gpu, findRows, plantings, cells, params, surface, features,
            groups: VIEWS.map(view => {
                const entries = [...this.instances.emitResources(view), { binding: 8, resource: { buffer: data } }, { binding: 9, resource: { buffer: params } }];
                return [
                    device.createBindGroup({ layout: findRows.getBindGroupLayout(0), entries: [...entries, { binding: 10, resource: { buffer: cells[view] } }] }),
                    device.createBindGroup({ layout: plantings.getBindGroupLayout(0), entries: [...entries, { binding: 11, resource: { buffer: cells[view] } }] }),
                ];
            }),
        };
    }

    /** The window of base cells about the eye at (lon, lat). */
    update(lonLat: [number, number]): void {
        const r = this.resources;
        if (!r) return;
        const [ux, uy] = worldPixel(lonLat, UNITS);
        this.side = this.sideAt(lonLat[1]);
        // Base cell b starts at (b + 1) × BASE, half a field cell past a whole one.
        const params = new Int32Array([Math.floor(ux / BASE) - 1 - this.side / 2, Math.floor(uy / BASE) - 1 - this.side / 2, this.side, 0]);
        new Float32Array(params.buffer)[3] = this.reach;
        r.gpu.device.queue.writeBuffer(r.params, 0, params);
        for (const cells of r.cells) r.gpu.device.queue.writeBuffer(cells, 0, new Uint32Array([0, 1, 1, 0]));
    }

    /** Base cells a side of the window about an eye at a latitude: the reach each way, and a cell more. */
    private sideAt(lat: number): number {
        return this.reach ? 2 * Math.ceil(this.reach / (BASE * anchorAt(0, lat).unit)) + 2 : 0;
    }

    encode(pass: GPUComputePassEncoder, ground: GPUBindGroup, view: number): void {
        const r = this.resources;
        if (!r || !this.side) return;
        pass.setBindGroup(0, r.groups[view][0]);
        pass.setBindGroup(1, r.surface);
        pass.setBindGroup(2, ground);
        pass.setBindGroup(3, r.features);
        pass.setPipeline(r.findRows);
        pass.dispatchWorkgroups(Math.ceil(this.side / 8), Math.ceil(this.side / 8));
        pass.setPipeline(r.plantings);
        pass.setBindGroup(0, r.groups[view][1]);
        pass.dispatchWorkgroupsIndirect(r.cells[view], 0);
    }
}

/** The base cells of the rows in view, and the plants in them. */
function rowsPipelines(gpu: Gpu): Promise<GPUComputePipeline[]> {
    const { device } = gpu, C = GPUShaderStage.COMPUTE, { surface, ground, features } = gpu.once(layouts);
    const shared: GPUBindGroupLayoutEntry[] = [
        ...Instances.emitEntries(), { binding: 8, visibility: C, buffer: { type: 'read-only-storage' } }, { binding: 9, visibility: C, buffer: { type: 'uniform' } },
    ];
    const module = device.createShaderModule({ label: 'rows', code: wgsl('vegetation/rows.wgsl') });
    return Promise.all(([['find_rows', { binding: 10, visibility: C, buffer: { type: 'storage' } }], ['plantings', { binding: 11, visibility: C, buffer: { type: 'read-only-storage' } }]] as const).map(([entryPoint, list]) => {
        const group = device.createBindGroupLayout({ entries: [...shared, list] });
        return device.createComputePipelineAsync({ label: entryPoint, layout: device.createPipelineLayout({ bindGroupLayouts: [group, surface, ground, features] }), compute: { module, entryPoint } });
    }));
}
