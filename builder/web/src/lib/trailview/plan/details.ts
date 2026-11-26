// Storey 0's track and step details near the eye (`mesh/details.rs` of obc-ground-plan): each part built in the
// vertex stage from its rings, corners of the cell's storey mesh (plan/details.wgsl). A level-18 node's parts draw
// while its square lies within REACH_M of the eye, and cast within CAST_M; from DETAIL_M on they fade out by the
// corridors' dither, and the bed's look paints the sleepers and rails past them (roads/track-looks.wgsl).
import type { Gpu } from '../gpu/device';
import { PIECE_IN } from '../gpu/include';
import { wgsl } from '../gpu/wgsl';
import { CASTER_DEPTH, CASTER_PRIMITIVE } from '../light/shadow';
import { DEPTH, sceneFormat } from '../resolve/targets';
import { distances, type Eye } from '../terrain/lod';
import type { StoreyMesh } from './mesh';

/** DETAIL_END_M of roads/track-looks.wgsl and a part's reach past its node; the casters' reach. */
const REACH_M = 194;
const CAST_M = 60;

const quads = (faces: number[][]) => faces.flatMap(([a, b, c, d]) => [a, b, c, a, c, d]);
/** A swept span's quads between the points of its section at its two rings, the last (its foot) left out. */
const sweep = (points: number) => quads(Array.from({ length: points - 1 }, (_, q) => [q, points + q, points + q + 1, q + 1]));
/**
 * Per draw (`Item::draw` of mesh/details.rs) the corners of a part's triangles, each beginning with the corner that
 * gives its look (plan/details.wgsl): a rail's span, a rack's or a cable's, a box (bit 2 its left side, bit 1 its front
 * end, bit 0 its top; no bottom), a cap (a rail's section in convex fans), a log (an arc of 8 points at each side, its
 * flat foot left out, and fans at its ends). No face left out is ever seen: each lies under the face or a part.
 */
const DRAWS = [
    sweep(12), sweep(4),
    quads([[5, 1, 3, 7], [0, 2, 3, 1], [6, 4, 5, 7], [2, 6, 7, 3], [4, 0, 1, 5]]),
    [0, 1, 2, 0, 2, 9, 0, 9, 10, 0, 10, 11, 2, 3, 8, 2, 8, 9, 3, 4, 5, 3, 5, 6, 3, 6, 7, 3, 7, 8],
    quads([...Array.from({ length: 7 }, (_, q) => [q, 8 + q, 9 + q, q + 1]), ...[1, 3, 5].flatMap(k => [[0, k, k + 1, k + 2], [10 + k, 9 + k, 8 + k, 8]])]),
];
const FIRSTS = DRAWS.map((_, d) => DRAWS.slice(0, d).reduce((n, x) => n + x.length, 0));

/** A pipeline of the details, the corners of their triangles and how far from the eye they draw. */
export interface DetailDraw { pipeline: GPURenderPipeline; corners: GPUBuffer; reach: number }
export interface DetailPipelines { draw: DetailDraw; cast: DetailDraw }

/** The draw and the caster of the details: groups 1 to 3 of each as the storeys' (pieces.ts). */
export async function detailPipelines(gpu: Gpu, layout: GPUBindGroupLayout, draw: GPUBindGroupLayout[], cast: GPUBindGroupLayout[]): Promise<DetailPipelines> {
    const { device } = gpu, module = device.createShaderModule({ label: 'plan details', code: wgsl('plan/details.wgsl', PIECE_IN) });
    const indices = new Uint16Array(DRAWS.flat()), corners = gpu.buffer('plan', indices.byteLength, GPUBufferUsage.INDEX | GPUBufferUsage.COPY_DST);
    device.queue.writeBuffer(corners, 0, indices);
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
    return { draw: { pipeline: drawn, corners, reach: REACH_M }, cast: { pipeline: caster, corners, reach: CAST_M } };
}

/** Draws the details of the cells (zoom 16, each with its storey mesh and the bind group of the pass) whose level-18 nodes lie within the draw's reach of the eye. Returns the triangles drawn. */
export function drawDetails(pass: GPURenderPassEncoder, { pipeline, corners, reach }: DetailDraw, eye: Eye, cells: { x: number; y: number; storeys: StoreyMesh; group: GPUBindGroup }[]): number {
    const ground = { anchor: eye.anchor, above: 0 };
    let triangles = 0;
    for (const { x, y, storeys, group } of cells) {
        let bound = false;
        for (const { node, draw, first, count } of storeys.ranges) {
            if (distances(ground, { level: 18, x: 4 * x + (node & 3), y: 4 * y + (node >> 2) })[0] >= reach) continue;
            if (!triangles) {
                pass.setPipeline(pipeline);
                pass.setIndexBuffer(corners, 'uint16');
            }
            if (!bound) pass.setBindGroup(0, group);
            bound = true;
            pass.drawIndexed(DRAWS[draw].length, count, FIRSTS[draw], 0, first);
            triangles += (count * DRAWS[draw].length) / 3;
        }
    }
    return triangles;
}
