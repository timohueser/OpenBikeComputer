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

interface Resources { gpu: Gpu; pipeline: GPUComputePipeline; groups: GPUBindGroup[]; params: GPUBuffer; surface: GPUBindGroup; features: GPUBindGroup }

/**
 * The plantings (rows.wgsl): the plants of the materials whose `rows` (materials.json) say how they are
 * planted, on the rows of the material fields, placed on the GPU each frame. A tree is a model of the
 * forest's (tree-models.ts), a plant one of the plants' (plants.ts).
 */
export class Rows implements Producer {
    private resources?: Resources;
    private readonly data: Uint32Array<ArrayBuffer>;
    private readonly reach: number;
    private side = 0;

    constructor(rules: Rules, trees: TreeModels, plants: Plants, private readonly instances: Instances) {
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
        const word = (region: Region, model: string | undefined) => {
            if (model && model in PLANTS) {
                const first = plants.rowModel(model);
                return first === undefined ? NO_MODEL : first | ALONG | (SEEDS << SHAPES);
            }
            const index = model && model in TREES ? trees.index(model, evergreen(region, model)) : undefined;
            return index === undefined ? NO_MODEL : index | (TREES[model!].keepsHeading ? WINDWARD : 0);
        };
        rules.regions.forEach((region, r) => crops.forEach(({ rows }, c) => (data[MODELS_AT + 4 * r + c] = word(region, rows.model ?? region.orchard))));
        this.data = data;
        this.reach = Math.max(0, ...crops.map(({ rows }) => rows.reach_m));
    }

    async attach(gpu: Gpu, { surface, features }: Surfaces): Promise<void> {
        this.resources = undefined;
        const { device } = gpu;
        const data = gpu.buffer('vegetation', this.data.byteLength, GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST);
        const params = gpu.buffer('vegetation', 16, GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST);
        device.queue.writeBuffer(data, 0, this.data);
        const pipeline = await gpu.once(rowsPipeline);
        this.resources = {
            gpu, pipeline, params, surface, features,
            groups: VIEWS.map(view => device.createBindGroup({ layout: pipeline.getBindGroupLayout(0), entries: [...this.instances.emitResources(view), { binding: 8, resource: { buffer: data } }, { binding: 9, resource: { buffer: params } }] })),
        };
    }

    /** The window of base cells about the eye at (lon, lat). */
    update(lonLat: [number, number]): void {
        const r = this.resources;
        if (!r) return;
        const [ux, uy] = worldPixel(lonLat, UNITS);
        this.side = this.reach ? 2 * Math.ceil(this.reach / (BASE * anchorAt(...lonLat).unit)) + 2 : 0;
        // Base cell b starts at (b + 1) × BASE, half a field cell past a whole one.
        const params = new Int32Array([Math.floor(ux / BASE) - 1 - this.side / 2, Math.floor(uy / BASE) - 1 - this.side / 2, this.side, 0]);
        new Float32Array(params.buffer)[3] = this.reach;
        r.gpu.device.queue.writeBuffer(r.params, 0, params);
    }

    encode(pass: GPUComputePassEncoder, ground: GPUBindGroup, view: number): void {
        const r = this.resources;
        if (!r || !this.side) return;
        pass.setBindGroup(0, r.groups[view]);
        pass.setBindGroup(1, r.surface);
        pass.setBindGroup(2, ground);
        pass.setBindGroup(3, r.features);
        pass.setPipeline(r.pipeline);
        pass.dispatchWorkgroups(this.side, this.side);
    }
}

function rowsPipeline(gpu: Gpu): Promise<GPUComputePipeline> {
    const { device } = gpu, C = GPUShaderStage.COMPUTE, { surface, ground, features } = gpu.once(layouts);
    const group = device.createBindGroupLayout({ entries: [
        ...Instances.emitEntries(), { binding: 8, visibility: C, buffer: { type: 'read-only-storage' } }, { binding: 9, visibility: C, buffer: { type: 'uniform' } },
    ] });
    const module = device.createShaderModule({ label: 'rows', code: wgsl('vegetation/rows.wgsl') });
    return device.createComputePipelineAsync({ label: 'plantings', layout: device.createPipelineLayout({ bindGroupLayouts: [group, surface, ground, features] }), compute: { module, entryPoint: 'plantings' } });
}
