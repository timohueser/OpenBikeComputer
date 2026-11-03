// The corridors' cover in plan, from which a way owns its ground: per stamp job of a frame that makes snaps, a render pass
// draws the near and the far faces of the L16 pieces over the job's page, then their borders (borders.ts), into one tile
// of a target (cover.wgsl), and the snap pass of terrain/stamp.wgsl reads it. So the terrain lies lower only where the
// faces of both ranges cover whole texels.
import type { Gpu } from '../gpu/device';
import { wgsl } from '../gpu/wgsl';
import { PAGE } from '../stream/heights';
import { CHILD, size, type Node } from '../terrain/lod';
import { ARRIVALS } from '../terrain/pages';
import type { Pool } from '../structures/pool';
import { ROAD_VERTEX, type RoadPiece } from './piece';

/** Texels round a page in its tile: the heights that a vertex at the page's edge drops under reach 5 past it. As COVER_MARGIN of stamp.wgsl and MARGIN of cover.wgsl. */
export const COVER_MARGIN = 8;
/** A tile's side and the tiles per row of the target, one tile per stamp job. As COVER_TILE and COVER_ROW of stamp.wgsl. */
export const COVER_TILE = PAGE + 2 * COVER_MARGIN, COVER_ROW = 4;
if (ARRIVALS > COVER_ROW ** 2) throw new Error(`The cover target holds ${COVER_ROW ** 2} tiles, not ${ARRIVALS}.`);
const SIDE = COVER_TILE * COVER_ROW;
/** Bytes of the heights of one tile. */
export const COVER_HEIGHT_BYTES = COVER_TILE * COVER_TILE * 4;
/** Draws of one frame at most, and the bytes of each one's instance: the piece's middle less the page's corner, the units of a texel, and the range. */
const DRAWS = 1024, INSTANCE = 16;
/** The ranges, as RANGE_NEAR of cover.wgsl. */
const NEAR = 1, FAR = 2;

/** The world-unit box [x0, y0, x1, y1] of what a piece's faces reach, with the widest margin of a tile (a texel of level CHILD). */
export function reachOf({ level, x, y, over }: RoadPiece): number[] {
    const r = size(level) / 2 + over + (COVER_MARGIN * size(CHILD)) / PAGE;
    return [x - r, y - r, x + r, y + r];
}

/**
 * What the snap pass reads: per texel whether near faces, far faces, a border's band and a way's surface cover it
 * (rgba8), and per tile in `heights` the least height of the faces at each texel, as cover.wgsl writes them.
 */
export function coverTargets(gpu: Gpu): { colour: GPUTexture; heights: GPUBuffer } {
    return {
        colour: gpu.texture('features', { size: [SIDE, SIDE], format: 'rgba8unorm', usage: GPUTextureUsage.RENDER_ATTACHMENT | GPUTextureUsage.TEXTURE_BINDING }, SIDE * SIDE * 4),
        heights: gpu.buffer('features', ARRIVALS * COVER_HEIGHT_BYTES, GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST),
    };
}

/** The cover pass's pipelines, once per device, the buffer of its instances, and the group of the heights that it writes. */
export interface Cover { faces: GPURenderPipeline; borders: GPURenderPipeline; instances: GPUBuffer; heights?: { buffer: GPUBuffer; group: GPUBindGroup } }

export const coverInstances = (gpu: Gpu) => gpu.buffer('roads', DRAWS * INSTANCE, GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST);

export async function coverPipelines(gpu: Gpu): Promise<Pick<Cover, 'faces' | 'borders'>> {
    const { device } = gpu, module = device.createShaderModule({ label: 'cover', code: wgsl('roads/cover.wgsl') });
    // Faces and bands that overlap keep each cover of either.
    const max = { operation: 'max', srcFactor: 'one', dstFactor: 'one' } as const, targets = [{ format: 'rgba8unorm', blend: { color: max, alpha: max } }] as const;
    const instance = (at: number): GPUVertexBufferLayout => ({ arrayStride: 0, stepMode: 'instance', attributes: [{ shaderLocation: at, offset: 0, format: 'sint32x2' }, { shaderLocation: at + 1, offset: 8, format: 'float32' }] });
    const [faces, borders] = await Promise.all([
        device.createRenderPipelineAsync({
            label: 'cover faces', layout: 'auto',
            vertex: {
                module, entryPoint: 'cover_vertex', buffers: [
                    { arrayStride: ROAD_VERTEX, attributes: [{ shaderLocation: 0, offset: 0, format: 'sint16x2' }, { shaderLocation: 1, offset: 4, format: 'float32' }, { shaderLocation: 2, offset: 8, format: 'uint32' }] },
                    { arrayStride: INSTANCE, stepMode: 'instance', attributes: [{ shaderLocation: 3, offset: 0, format: 'sint32x2' }, { shaderLocation: 4, offset: 8, format: 'float32' }, { shaderLocation: 5, offset: 12, format: 'uint32' }] },
                ],
            },
            fragment: { module, entryPoint: 'cover_fragment', targets },
            primitive: { topology: 'triangle-list', cullMode: 'none' },
        }),
        device.createRenderPipelineAsync({
            label: 'cover borders', layout: 'auto',
            // A border per instance from the piece's slots; the draw's instance, which all of its borders share, at the offset of its buffer.
            vertex: { module, entryPoint: 'border_vertex', buffers: [{ arrayStride: ROAD_VERTEX, stepMode: 'instance', attributes: [{ shaderLocation: 0, offset: 0, format: 'sint32x4' }] }, instance(1)] },
            fragment: { module, entryPoint: 'border_fragment', targets },
            primitive: { topology: 'triangle-list', cullMode: 'none' },
        }),
    ]);
    return { faces, borders };
}

/**
 * Draws into tile k of the target, which the pass clears, the faces and the borders of the pieces of `jobs[k]` over its
 * page; a job without pieces leaves its tile empty. The faces write their heights into `heights`, cleared to 0.
 */
export function drawCover(pass: GPURenderPassEncoder, device: GPUDevice, cover: Cover, pool: Pool, heights: GPUBuffer, jobs: ({ node: Node; pieces: readonly RoadPiece[] } | undefined)[]): void {
    const draws: { k: number; piece: RoadPiece; at: { first: number; base: number } }[] = [];
    jobs.forEach((job, k) => {
        for (const piece of job?.pieces ?? []) {
            const at = pool.at(piece);
            if (at && 2 * draws.length + 2 <= DRAWS) draws.push({ k, piece, at });
        }
    });
    if (!draws.length) return;
    const bytes = new ArrayBuffer(2 * draws.length * INSTANCE), ints = new Int32Array(bytes), floats = new Float32Array(bytes), words = new Uint32Array(bytes);
    draws.forEach(({ k, piece }, i) => {
        const { level, x, y } = jobs[k]!.node;
        for (const [n, range] of [[2 * i, NEAR], [2 * i + 1, FAR]]) {
            ints.set([piece.x - x * size(level), piece.y - y * size(level)], 4 * n);
            floats[4 * n + 2] = size(level) / PAGE;
            words[4 * n + 3] = range;
        }
    });
    device.queue.writeBuffer(cover.instances, 0, bytes);
    if (cover.heights?.buffer !== heights) cover.heights = { buffer: heights, group: device.createBindGroup({ layout: cover.faces.getBindGroupLayout(0), entries: [{ binding: 0, resource: { buffer: heights } }] }) };
    const tiles = new Map<number, number[]>();
    draws.forEach(({ k }, i) => (tiles.get(k) ?? tiles.set(k, []).get(k)!).push(i));
    for (const [k, list] of tiles) {
        const [x, y] = [(k % COVER_ROW) * COVER_TILE, Math.floor(k / COVER_ROW) * COVER_TILE];
        pass.setViewport(x, y, COVER_TILE, COVER_TILE, 0, 1);
        pass.setScissorRect(x, y, COVER_TILE, COVER_TILE);
        pass.setPipeline(cover.faces);
        pass.setBindGroup(0, cover.heights.group);
        pass.setVertexBuffer(0, pool.vertices);
        pass.setVertexBuffer(1, cover.instances);
        pass.setIndexBuffer(pool.indices, 'uint16');
        for (const i of list) {
            const { piece: { near, far }, at } = draws[i];
            pass.drawIndexed(near.count, 1, at.first + near.first, at.base, 2 * i);
            pass.drawIndexed(far.count, 1, at.first + far.first, at.base, 2 * i + 1);
        }
        pass.setPipeline(cover.borders);
        for (const i of list) {
            const { piece: { borders }, at } = draws[i];
            pass.setVertexBuffer(1, cover.instances, 2 * i * INSTANCE);
            for (const { first, count } of [borders.near, borders.far]) if (count) pass.draw(6, count, 0, at.base + first);
        }
    }
}
