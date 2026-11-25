// Storey 0's track and step details near the eye (`mesh/details.rs` of obc-ground-plan): each part built in the
// vertex stage from its rings, corners of the cell's storey mesh (plan/details.wgsl). A level-18 node's parts draw and
// cast while its square lies within REACH_M of the eye; from DETAIL_M on they fade out by the corridors' dither, and
// the bed's look paints the sleepers and rails past them (roads/track-looks.wgsl).
import type { Gpu } from '../gpu/device';
import { PIECE_IN } from '../gpu/include';
import { wgsl } from '../gpu/wgsl';
import { CASTER_DEPTH, CASTER_PRIMITIVE } from '../light/shadow';
import { DEPTH, sceneFormat } from '../resolve/targets';
import { distances, type Eye } from '../terrain/lod';
import type { StoreyMesh } from './mesh';

/** DETAIL_END_M of roads/track-looks.wgsl and a part's reach past its node. */
const REACH_M = 194;
/** The vertices of a part of each draw: VERTICES of mesh/details.rs. */
const VERTICES = [72, 36, 84];

export interface DetailPipelines { draw: GPURenderPipeline; cast: GPURenderPipeline }

/** The draw and the caster of the details: groups 1 to 3 of each as the storeys' (pieces.ts). */
export async function detailPipelines(gpu: Gpu, layout: GPUBindGroupLayout, draw: GPUBindGroupLayout[], cast: GPUBindGroupLayout[]): Promise<DetailPipelines> {
    const { device } = gpu, module = device.createShaderModule({ label: 'plan details', code: wgsl('plan/details.wgsl', PIECE_IN) });
    const [drawn, caster] = await Promise.all([
        device.createRenderPipelineAsync({
            label: 'plan details', layout: device.createPipelineLayout({ bindGroupLayouts: [layout, ...draw] }), vertex: { module, entryPoint: 'detail_vertex' },
            // PIECE off: the faces shade by their facets and fade near DETAIL_END_M, as the corridors' parts.
            fragment: { module, entryPoint: 'corridor_fragment', targets: [{ format: sceneFormat(gpu) }], constants: { BAND: 1, MODEL: 1, PLAN_DETAILS: 1 } },
            primitive: { topology: 'triangle-list', cullMode: 'back' }, depthStencil: { format: DEPTH, depthWriteEnabled: true, depthCompare: 'greater' },
        }),
        device.createRenderPipelineAsync({
            label: 'plan detail caster', layout: device.createPipelineLayout({ bindGroupLayouts: [layout, ...cast] }), vertex: { module, entryPoint: 'detail_caster' },
            primitive: CASTER_PRIMITIVE, depthStencil: CASTER_DEPTH,
        }),
    ]);
    return { draw: drawn, cast: caster };
}

/** Draws the details of the cells (zoom 16, each with its storey mesh and the bind group of the pass) whose level-18 nodes lie near the eye. Returns the parts drawn. */
export function drawDetails(pass: GPURenderPassEncoder, pipeline: GPURenderPipeline, eye: Eye, cells: { x: number; y: number; storeys: StoreyMesh; group: GPUBindGroup }[]): number {
    const ground = { anchor: eye.anchor, above: 0 };
    let parts = 0;
    for (const { x, y, storeys, group } of cells) {
        let bound = false;
        for (const { node, draw, first, count } of storeys.ranges) {
            if (distances(ground, { level: 18, x: 4 * x + (node & 3), y: 4 * y + (node >> 2) })[0] >= REACH_M) continue;
            if (!parts) pass.setPipeline(pipeline);
            if (!bound) pass.setBindGroup(0, group);
            bound = true;
            pass.draw(VERTICES[draw], count, 0, first);
            parts += count;
        }
    }
    return parts;
}
