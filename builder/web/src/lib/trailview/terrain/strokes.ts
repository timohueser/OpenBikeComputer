// Spike (`far=strokes`): the ways of the pages of level 16 and finer drawn from the record lines of their cell
// (strokes.wgsl) instead of looked up per texel (model.wgsl, heights.wgsl). The stamp pass reads the drawn
// atlas. `pieces=off` hides the near meshes, so the drawn look shows at the eye.
import type { Gpu } from '../gpu/device';
import { wgsl } from '../gpu/wgsl';
import { PAGE } from '../stream/heights';
import { CELL_LEVEL } from './model';
import { key, type Node } from './lod';

const query = typeof location === 'undefined' ? new URLSearchParams() : new URLSearchParams(location.search);
export const FAR_STROKES = query.get('far') === 'strokes';
export const HIDE_PIECES = query.get('pieces') === 'off';
/** The bit of a stamp job's mode (stamp.wgsl) whose page reads the drawn strokes. */
export const STROKED = 8;

/** Tiles per side of the atlas: one per stamp job of a frame (ARRIVALS). */
const TILES = 4;
export const ATLAS = TILES * PAGE;
const UNIFORM = 256;
const JOBS = TILES * TILES;
/** As records.rs: the words of a candidate and of a line, the candidate kind of a way, the line flag of a rail. */
const CAND = 11, LINE = 16, WAY = 0, RAIL = 1 << 8;
/** Instance flags above the sample index: the line's first or last end is a cut (strokes.wgsl). */
const CUT_START = 1 << 16, CUT_END = 1 << 17;

/** A cell's instances: per centre-line segment its line, its first sample and the key ranks of both samples. */
export interface CellStrokes { node: Node; block: number; boxes: Float64Array; instances: Uint32Array<ArrayBuffer>; count: number; keys: number; base: number; rules: Float32Array; lineWords: number; sampleWords: number; blockWords: number }

/** The instances of a records block (`faces::records`): every segment of every record line. */
export function strokesOf(node: Node, words: Uint32Array, block: number): CellStrokes {
    const floats = new Float32Array(words.buffer, words.byteOffset, words.length);
    const cands = words[1], lines = words[3], candAt = words[4], lineAt = words[6], sampleAt = words[7], sample = sampleWords(words);
    // The candidate key of each line's carriageway or rail bed; a line with no candidate in the window takes its class's.
    const keyOf = new Map<number, number>(), rankOf = new Map<boolean, number>();
    for (let c = 0; c < cands; c++) {
        const at = candAt + CAND * c;
        if (words[at] !== WAY) continue;
        keyOf.set(words[at + 4], words[at + 1]);
        rankOf.set((words[lineAt + LINE * words[at + 4] + 1] & RAIL) !== 0, words[at + 1] >>> 16);
    }
    const lineKey = (l: number) => keyOf.get(l) ?? (((rankOf.get((words[lineAt + LINE * l + 1] & RAIL) !== 0) ?? 0xff) << 16) | ((words[lineAt + LINE * l + 7] & 0xff) << 8) | 3);
    // The overlay's order: the key, then the glob order of the sample (records::decide, way_glob).
    const pairs: number[] = [];
    for (let l = 0; l < lines; l++) {
        const first = words[lineAt + LINE * l + 2], n = words[lineAt + LINE * l + 3], k = lineKey(l);
        for (let i = 0; i < n; i++) pairs.push(k * 2 ** 20 + words[sampleAt + sample * (first + i) + 4]);
    }
    const ranks = [...new Set(pairs)].sort((a, b) => a - b), rank = new Map(ranks.map((v, i) => [v, i]));
    const out: number[] = [], boxes: number[] = [];
    // A line ends square where its way goes on as its next leg (the crate's cut ends), as at a level change.
    const way = (l: number) => words[lineAt + LINE * l], leg = (l: number) => words[lineAt + LINE * l + 1] >>> 24;
    let at = 0;
    for (let l = 0; l < lines; l++) {
        const first = words[lineAt + LINE * l + 2], n = words[lineAt + LINE * l + 3];
        const cuts = (leg(l) > 0 ? CUT_START : 0) | (l + 1 < lines && way(l + 1) === way(l) && leg(l + 1) === leg(l) + 1 ? CUT_END : 0);
        for (let i = 0; i + 1 < n; i++) {
            const a = sampleAt + sample * (first + i), b = a + sample;
            if (words[a] === words[b] && words[a + 1] === words[b + 1]) continue;
            out.push(l, i | cuts, rank.get(pairs[at + i])!, rank.get(pairs[at + i + 1])!);
            const ax = words[a] | 0, ay = words[a + 1] | 0, bx = words[b] | 0, by = words[b + 1] | 0;
            boxes.push(Math.min(ax, bx), Math.min(ay, by), Math.max(ax, bx), Math.max(ay, by));
        }
        at += n;
    }
    const total = words[sampleAt + 5] !== undefined && lines ? floats[sampleAt + 5] : 0;
    return {
        node, block, boxes: Float64Array.from(boxes), instances: new Uint32Array(out), count: out.length / 4, keys: Math.max(1, ranks.length), base: Number.isFinite(total) ? total : 0,
        rules: floats.slice(15, 15 + 24), lineWords: LINE * lines, sampleWords: (words[8] - sampleAt), blockWords: words.length,
    };
}

/** The words per sample of a block: the sample table's words over its samples. */
function sampleWords(words: Uint32Array): number {
    const lines = words[3], lineAt = words[6];
    let samples = 0;
    for (let l = 0; l < lines; l++) samples = Math.max(samples, words[lineAt + LINE * l + 2] + words[lineAt + LINE * l + 3]);
    return samples ? (words[8] - words[7]) / samples : 14;
}

interface Targets { owner: GPUTexture; frame: GPUTexture; dist: GPUTexture; lower: GPUTexture; upper: GPUTexture; depth: GPUTexture }
interface Pipelines { owner: GPURenderPipeline; dist: GPURenderPipeline; fill: GPURenderPipeline; cut: GPURenderPipeline; layout0: GPUBindGroupLayout; layout1: GPUBindGroupLayout }

/** A stamp job that the strokes draw: its index in the frame's jobs, its node. */
export interface StrokeJob { k: number; node: Node }

export class Strokes {
    private cells = new Map<string, CellStrokes & { buffer?: GPUBuffer }>();
    private gpu?: Gpu;
    private targets?: Targets;
    private pipelines?: Pipelines;
    private uniform?: GPUBuffer;
    private group0?: GPUBindGroup;
    private group1?: GPUBindGroup;
    /** The instances drawn per job in the last frame, for the numbers. */
    drawn: { node: string; instances: number }[] = [];

    add(node: Node, words: Uint32Array, block: number): void {
        this.remove(node);
        this.cells.set(key(node), strokesOf(node, words, block));
    }

    remove(node: Node): void {
        const buffer = this.cells.get(key(node))?.buffer;
        if (buffer) this.gpu?.destroy('strokes', buffer);
        this.cells.delete(key(node));
    }

    /** The instances of a page's cell whose quad can reach the page: within `metres` (about 40 m) of its square. */
    touching(node: Node, metres = 40): { cell: number; page: number } {
        const shift = node.level - CELL_LEVEL, cell = this.cells.get(key({ level: CELL_LEVEL, x: node.x >> shift, y: node.y >> shift }));
        if (!cell) return { cell: 0, page: 0 };
        const side = 2 ** (18 - shift), x0 = (node.x - ((node.x >> shift) << shift)) * side, y0 = (node.y - ((node.y >> shift) << shift)) * side, pad = metres / 0.0016;
        let page = 0;
        for (let i = 0; i < cell.count; i++) {
            const b = cell.boxes.subarray(4 * i, 4 * i + 4);
            if (b[2] >= x0 - pad && b[0] <= x0 + side + pad && b[3] >= y0 - pad && b[1] <= y0 + side + pad) page++;
        }
        return { cell: cell.count, page };
    }

    has(node: Node): boolean {
        return this.cells.has(key(node));
    }

    get stats(): { cells: number; instances: number; instanceBytes: number; lineSampleBytes: number; blockBytes: number } {
        const all = [...this.cells.values()];
        return {
            cells: all.length, instances: all.reduce((s, c) => s + c.count, 0), instanceBytes: all.reduce((s, c) => s + c.count * 4, 0),
            lineSampleBytes: all.reduce((s, c) => s + 4 * (c.lineWords + c.sampleWords), 0), blockBytes: all.reduce((s, c) => s + 4 * c.blockWords, 0),
        };
    }

    /** Takes the targets and the pipelines on a device; `model` is the records texture. Returns the views that the stamp binds. */
    attach(gpu: Gpu, model: GPUTexture): GPUTextureView[] {
        this.gpu = gpu;
        for (const cell of this.cells.values()) cell.buffer = undefined;
        const usage = GPUTextureUsage.RENDER_ATTACHMENT | GPUTextureUsage.TEXTURE_BINDING;
        const size = FAR_STROKES ? [ATLAS, ATLAS] : [1, 1], texels = size[0] * size[1];
        const make = (format: GPUTextureFormat, bytes: number) => gpu.texture('strokes', { size, format, usage }, bytes * texels);
        const t = this.targets = { owner: make('r32uint', 4), frame: make('rgba32float', 16), dist: make('rgba16float', 8), lower: make('r32float', 4), upper: make('r32float', 4), depth: make('depth32float', 4) };
        if (FAR_STROKES) {
            const p = this.pipelines = gpu.once(strokePipelines);
            this.uniform = gpu.buffer('strokes', UNIFORM * JOBS, GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST);
            this.group0 = gpu.device.createBindGroup({ layout: p.layout0, entries: [{ binding: 0, resource: { buffer: this.uniform, size: UNIFORM } }, { binding: 8, resource: model.createView() }] });
            this.group1 = gpu.device.createBindGroup({ layout: p.layout1, entries: [{ binding: 0, resource: t.lower.createView() }] });
        }
        return [t.owner, t.frame, t.dist, t.lower, t.upper].map(texture => texture.createView());
    }

    /** Encodes the four passes of the frame's jobs of level 16 and finer whose cell has its block. */
    encode(encoder: GPUCommandEncoder, jobs: StrokeJob[], time: (pass: string) => GPURenderPassTimestampWrites | undefined = () => undefined): StrokeJob[] {
        const { gpu, targets: t, pipelines: p } = this;
        if (!FAR_STROKES || !gpu || !t || !p) return [];
        const cellKey = (node: Node) => key({ level: CELL_LEVEL, x: node.x >> (node.level - CELL_LEVEL), y: node.y >> (node.level - CELL_LEVEL) });
        const drawn = jobs.filter(({ node }) => node.level >= CELL_LEVEL && this.cells.has(cellKey(node)));
        this.drawn = drawn.map(({ node }) => ({ node: key(node), instances: this.cells.get(cellKey(node))!.count }));
        if (!drawn.length) return [];
        const data = new ArrayBuffer(UNIFORM * JOBS), ints = new Int32Array(data), floats = new Float32Array(data), uints = new Uint32Array(data);
        for (const { k, node } of drawn) {
            const cell = this.cells.get(cellKey(node))!, w = (UNIFORM / 4) * k;
            const shift = node.level - CELL_LEVEL, upt = 2 ** (26 - node.level);
            // The page's corner in frame units from the cell's origin.
            ints.set([(node.x - ((node.x >> shift) << shift)) * PAGE * upt, (node.y - ((node.y >> shift) << shift)) * PAGE * upt], w);
            floats.set([PAGE * (k % TILES), PAGE * Math.floor(k / TILES), upt], w + 2);
            uints[w + 5] = cell.block;
            floats.set([cell.keys, cell.base], w + 6);
            floats.set(cell.rules, w + 8);
            if (!cell.buffer) {
                cell.buffer = gpu.buffer('strokes', Math.max(16, cell.instances.byteLength), GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST);
                gpu.device.queue.writeBuffer(cell.buffer, 0, cell.instances);
            }
        }
        gpu.device.queue.writeBuffer(this.uniform!, 0, data);
        const draw = (pass: GPURenderPassEncoder, pipeline: GPURenderPipeline, cut = false) => {
            pass.setPipeline(pipeline);
            if (cut) pass.setBindGroup(1, this.group1!);
            for (const { k, node } of drawn) {
                const cell = this.cells.get(cellKey(node))!, x = PAGE * (k % TILES), y = PAGE * Math.floor(k / TILES);
                pass.setViewport(x, y, PAGE, PAGE, 0, 1);
                pass.setScissorRect(x, y, PAGE, PAGE);
                pass.setBindGroup(0, this.group0!, [UNIFORM * k]);
                pass.setVertexBuffer(0, cell.buffer!);
                if (cell.count) pass.draw(4, cell.count);
            }
            pass.end();
        };
        const colour = (texture: GPUTexture, clear: GPUColor): GPURenderPassColorAttachment => ({ view: texture.createView(), loadOp: 'clear', storeOp: 'store', clearValue: clear });
        const depth = (clear: number): GPURenderPassDepthStencilAttachment => ({ view: t.depth.createView(), depthLoadOp: 'clear', depthStoreOp: 'discard', depthClearValue: clear });
        draw(encoder.beginRenderPass({ label: 'strokes owner', colorAttachments: [colour(t.frame, [0, 0, 0, 0]), colour(t.owner, [0xffffffff, 0, 0, 0])], depthStencilAttachment: depth(1), timestampWrites: time('strokes-owner') }), p.owner);
        draw(encoder.beginRenderPass({ label: 'strokes distances', colorAttachments: [colour(t.dist, [60000, 60000, 60000, 60000])], timestampWrites: time('strokes-dist') }), p.dist);
        draw(encoder.beginRenderPass({ label: 'strokes fill', colorAttachments: [colour(t.lower, [-1e9, 0, 0, 0])], depthStencilAttachment: depth(0), timestampWrites: time('strokes-fill') }), p.fill);
        draw(encoder.beginRenderPass({ label: 'strokes cut', colorAttachments: [colour(t.upper, [1e9, 0, 0, 0])], depthStencilAttachment: depth(1), timestampWrites: time('strokes-cut') }), p.cut, true);
        return drawn;
    }
}

function strokePipelines(gpu: Gpu): Pipelines {
    const { device } = gpu, VF = GPUShaderStage.VERTEX | GPUShaderStage.FRAGMENT;
    const layout0 = device.createBindGroupLayout({ entries: [{ binding: 0, visibility: VF, buffer: { type: 'uniform', hasDynamicOffset: true } }, { binding: 8, visibility: VF, texture: { sampleType: 'uint' } }] });
    const layout1 = device.createBindGroupLayout({ entries: [{ binding: 0, visibility: GPUShaderStage.FRAGMENT, texture: { sampleType: 'unfilterable-float' } }] });
    const module = device.createShaderModule({ label: 'strokes', code: wgsl('terrain/strokes.wgsl') });
    const buffers: GPUVertexBufferLayout[] = [{ arrayStride: 16, stepMode: 'instance', attributes: [{ shaderLocation: 0, format: 'uint32x4', offset: 0 }] }];
    const pipeline = (label: string, pass: number, entryPoint: string, targets: GPUColorTargetState[], depth?: GPUCompareFunction, groups = [layout0]) => device.createRenderPipeline({
        label, layout: device.createPipelineLayout({ bindGroupLayouts: groups }),
        vertex: { module, entryPoint: 'stroke_vertex', buffers, constants: { PASS: pass } },
        fragment: { module, entryPoint, targets },
        primitive: { topology: 'triangle-strip' },
        ...(depth ? { depthStencil: { format: 'depth32float', depthWriteEnabled: true, depthCompare: depth } } : {}),
    });
    const min: GPUBlendComponent = { operation: 'min', srcFactor: 'one', dstFactor: 'one' };
    return {
        owner: pipeline('strokes owner', 0, 'owner_fragment', [{ format: 'rgba32float' }, { format: 'r32uint' }], 'less'),
        dist: pipeline('strokes distances', 1, 'distance_fragment', [{ format: 'rgba16float', blend: { color: min, alpha: min } }]),
        fill: pipeline('strokes fill', 2, 'fill_fragment', [{ format: 'r32float' }], 'greater'),
        cut: pipeline('strokes cut', 2, 'cut_fragment', [{ format: 'r32float' }], 'less', [layout0, layout1]),
        layout0, layout1,
    };
}

/** Spike: the raw bytes of texture rectangles (rows without padding) and buffer ranges, for the difference maps. */
export async function readRegions(device: GPUDevice, regions: ({ texture: GPUTexture; origin: [number, number, number]; size: [number, number]; texelBytes: number } | { buffer: GPUBuffer; offset: number; bytes: number })[]): Promise<Uint8Array[]> {
    const align = (bytes: number) => Math.ceil(bytes / 256) * 256;
    const sizes = regions.map(r => ('texture' in r ? align(r.size[0] * r.texelBytes) * r.size[1] : align(r.bytes)));
    const staging = device.createBuffer({ size: Math.max(256, sizes.reduce((a, b) => a + b, 0)), usage: GPUBufferUsage.MAP_READ | GPUBufferUsage.COPY_DST });
    const encoder = device.createCommandEncoder();
    let at = 0;
    regions.forEach((r, k) => {
        if ('texture' in r) encoder.copyTextureToBuffer({ texture: r.texture, origin: r.origin }, { buffer: staging, offset: at, bytesPerRow: align(r.size[0] * r.texelBytes) }, [...r.size, 1]);
        else encoder.copyBufferToBuffer(r.buffer, r.offset, staging, at, r.bytes);
        at += sizes[k];
    });
    device.queue.submit([encoder.finish()]);
    await staging.mapAsync(GPUMapMode.READ);
    const all = new Uint8Array(staging.getMappedRange());
    at = 0;
    const out = regions.map((r, k) => {
        const start = at;
        at += sizes[k];
        if (!('texture' in r)) return all.slice(start, start + r.bytes);
        const row = align(r.size[0] * r.texelBytes), used = r.size[0] * r.texelBytes, bytes = new Uint8Array(used * r.size[1]);
        for (let y = 0; y < r.size[1]; y++) bytes.set(all.subarray(start + y * row, start + y * row + used), y * used);
        return bytes;
    });
    staging.unmap();
    staging.destroy();
    return out;
}
