// The corridors' cover in plan, from which a way owns its ground: per stamp job of a frame that makes snaps, a render pass
// draws the near faces of the L16 pieces that draw over the job's page into one tile of a target (cover.wgsl), and the
// snap pass of terrain/stamp.wgsl reads it. So the terrain lies lower only where a face draws over it.
import type { Gpu } from '../gpu/device';
import { wgsl } from '../gpu/wgsl';
import { PAGE } from '../stream/heights';
import { CHILD, size, type Node } from '../terrain/lod';
import { ARRIVALS } from '../terrain/pages';
import type { Pool } from '../structures/pool';
import { ROAD_VERTEX, type RoadPiece } from './piece';

/** Texels round a page in its tile: the one-ring of a vertex at the page's edge reaches 4. As COVER_MARGIN of stamp.wgsl and MARGIN of cover.wgsl. */
export const COVER_MARGIN = 8;
/** A tile's side and the tiles per row of the target, one tile per stamp job. As COVER_TILE and COVER_ROW of stamp.wgsl. */
export const COVER_TILE = PAGE + 2 * COVER_MARGIN, COVER_ROW = 4;
if (ARRIVALS > COVER_ROW ** 2) throw new Error(`The cover target holds ${COVER_ROW ** 2} tiles, not ${ARRIVALS}.`);
const SIDE = COVER_TILE * COVER_ROW;
/** Draws of one frame at most, and the bytes of each one's instance: the piece's middle less the page's corner, and the units of a texel. */
const DRAWS = 1024, INSTANCE = 12;

/** The world-unit box [x0, y0, x1, y1] of what a piece's faces reach, with the widest margin of a tile (a texel of level CHILD). */
export function reachOf({ level, x, y, over }: RoadPiece): number[] {
    const r = size(level) / 2 + over + (COVER_MARGIN * size(CHILD)) / PAGE;
    return [x - r, y - r, x + r, y + r];
}

/** Whether a box meets the square of a node. */
const meets = ([x0, y0, x1, y1]: number[], { level, x, y }: Node) => x1 > x * size(level) && x0 < (x + 1) * size(level) && y1 > y * size(level) && y0 < (y + 1) * size(level);

/** The target that the snap pass reads: whether a face covers a texel and is a way's surface (rg8), and the least height of the faces (depth). */
export function coverTargets(gpu: Gpu): { colour: GPUTexture; depth: GPUTexture } {
    const usage = GPUTextureUsage.RENDER_ATTACHMENT | GPUTextureUsage.TEXTURE_BINDING;
    return {
        colour: gpu.texture('features', { size: [SIDE, SIDE], format: 'rg8unorm', usage }, SIDE * SIDE * 2),
        depth: gpu.texture('features', { size: [SIDE, SIDE], format: 'depth32float', usage }, SIDE * SIDE * 4),
    };
}

/** The cover pass's pipeline, once per device, and the buffer of its instances. */
export interface Cover { pipeline: GPURenderPipeline; instances: GPUBuffer }

export const coverInstances = (gpu: Gpu) => gpu.buffer('roads', DRAWS * INSTANCE, GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST);

export function coverPipeline(gpu: Gpu): Promise<GPURenderPipeline> {
    const { device } = gpu, module = device.createShaderModule({ label: 'cover', code: wgsl('roads/cover.wgsl') });
    return device.createRenderPipelineAsync({
        label: 'cover', layout: 'auto',
        vertex: {
            module, entryPoint: 'cover_vertex', buffers: [
                { arrayStride: ROAD_VERTEX, attributes: [{ shaderLocation: 0, offset: 0, format: 'sint16x2' }, { shaderLocation: 1, offset: 4, format: 'float32' }, { shaderLocation: 2, offset: 8, format: 'uint32' }] },
                { arrayStride: INSTANCE, stepMode: 'instance', attributes: [{ shaderLocation: 3, offset: 0, format: 'sint32x2' }, { shaderLocation: 4, offset: 8, format: 'float32' }] },
            ],
        },
        // Faces that overlap keep the cover and the surface of either, and the lower height.
        fragment: { module, entryPoint: 'cover_fragment', targets: [{ format: 'rg8unorm', blend: { color: { operation: 'max', srcFactor: 'one', dstFactor: 'one' }, alpha: { operation: 'max', srcFactor: 'one', dstFactor: 'one' } } }] },
        primitive: { topology: 'triangle-list', cullMode: 'none' },
        depthStencil: { format: 'depth32float', depthWriteEnabled: true, depthCompare: 'less-equal' },
    });
}

/**
 * Draws into tile k of the target, which the pass clears, the near faces of each of `pieces` whose reach meets the page of
 * `pages[k]`; a job without a page leaves its tile empty.
 */
export function drawCover(pass: GPURenderPassEncoder, device: GPUDevice, cover: Cover, pool: Pool, pieces: RoadPiece[], pages: (Node | undefined)[]): void {
    const boxes = pieces.map(reachOf), draws: { k: number; piece: RoadPiece }[] = [];
    pages.forEach((node, k) => {
        if (node) pieces.forEach((piece, i) => draws.length < DRAWS && meets(boxes[i], node) && draws.push({ k, piece }));
    });
    if (!draws.length) return;
    const bytes = new ArrayBuffer(draws.length * INSTANCE), ints = new Int32Array(bytes), floats = new Float32Array(bytes);
    draws.forEach(({ k, piece }, i) => {
        const { level, x, y } = pages[k]!;
        ints.set([piece.x - x * size(level), piece.y - y * size(level)], 3 * i);
        floats[3 * i + 2] = size(level) / PAGE;
    });
    device.queue.writeBuffer(cover.instances, 0, bytes);
    pass.setPipeline(cover.pipeline);
    pass.setVertexBuffer(0, pool.vertices);
    pass.setVertexBuffer(1, cover.instances);
    pass.setIndexBuffer(pool.indices, 'uint16');
    let tile = -1;
    draws.forEach(({ k, piece }, i) => {
        const at = pool.at(piece);
        if (!at || !piece.near.count) return;
        if (k !== tile) {
            const [x, y] = [(k % COVER_ROW) * COVER_TILE, Math.floor(k / COVER_ROW) * COVER_TILE];
            pass.setViewport(x, y, COVER_TILE, COVER_TILE, 0, 1);
            pass.setScissorRect(x, y, COVER_TILE, COVER_TILE);
        }
        tile = k;
        pass.drawIndexed(piece.near.count, 1, at.first + piece.near.first, at.base, i);
    });
}
