// The ground plan's meshes near the eye (`ground=plan`): each cell's mesh from the plan worker on the device,
// its corners' heights, normals and looks made once by a compute pass (corners.wgsl), drawn before the
// terrain with the corridors' fragment stage (piece.wgsl). A cell's mesh draws over its level-18 nodes; the
// terrain's level-18 vertices there take no snap of their own and sink inside the mesh's owned region
// (cells.wgsl), and the strip, made of the terrain's own vertices at the region's border, closes the mesh
// against it. Past them the terrain draws the records' far look.
import type { Gpu } from '../gpu/device';
import { layouts } from '../gpu/layouts';
import { wgsl } from '../gpu/wgsl';
import { DEPTH, sceneFormat } from '../resolve/targets';
import { distances, key, size, type Eye, type Node, type Rule } from '../terrain/lod';
import { cellMesh, SUNK_WORDS, type CellMesh } from './mesh';

/** The cells per side of the window about the eye's cell, the words before the slots, and the slots of sunk points: as cells.wgsl. */
export const WINDOW = 7;
const HEAD = 64;
const SLOTS = 24;
/** The bytes of the cells buffer that the terrain and the pieces read. */
export const CELLS_BYTES = 4 * (HEAD + SLOTS * SUNK_WORDS);
const CELL_LEVEL = 16;
/** A cell's mesh builds this far before the terrain draws the whole cell at level 18: about 10 s of a car. */
const LEAD_M = 150;
/** Corners whose heights the compute pass makes in a frame at most, past the first piece: about a town cell, five heights each. */
const CORNERS_PER_FRAME = 70_000;
/** Bytes of a piece's parameters, as Piece of corners.wgsl. */
const PARAMS_BYTES = 32;

interface Piece {
    node: Node;
    mesh: CellMesh;
    /** The first word of the cell's records block that its labels index. */
    block: number;
    gpu?: { data: GPUBuffer; indices: GPUBuffer; params: GPUBuffer; draw: GPUBindGroup; compute: GPUBindGroup };
    /** Whether its corners hold their heights, and whether they must be made again. */
    computed: boolean;
    stale: boolean;
}

interface Resources {
    gpu: Gpu;
    frame: GPUBuffer;
    cells: GPUBuffer;
    records: GPUTexture;
    /** The triangles and the step slots of a cell drawn wholly at level 18, and of one drawn in part (`cut`). */
    triangles: GPURenderPipeline;
    slots: GPURenderPipeline;
    cutTriangles: GPURenderPipeline;
    cutSlots: GPURenderPipeline;
    corners: GPUComputePipeline;
    drawLayout: GPUBindGroupLayout;
    computeLayout: GPUBindGroupLayout;
}

export interface PiecesStats {
    /** Cells whose mesh is on the device, that draw, and that draw in view; their triangles and step slots in view. */
    cells: number;
    drawn: number;
    inView: number;
    triangles: number;
    steps: number;
    /** The pieces' GPU bytes. */
    bytes: number;
    /** Milliseconds from the first frame with a wanted cell that does not draw to the next frame in which all draw. */
    ready: number;
}

/** The cells of zoom 16 that hold a point near the eye: those within `reach` cells of the eye's. */
function cellsAbout(eye: Eye, reach: number): Node[] {
    const s = size(CELL_LEVEL), x = Math.floor((eye.anchor.whole[0] + eye.anchor.part[0]) / s), y = Math.floor((eye.anchor.whole[1] + eye.anchor.part[1]) / s);
    const out: Node[] = [];
    for (let j = -reach; j <= reach; j++) for (let i = -reach; i <= reach; i++) out.push({ level: CELL_LEVEL, x: x + i, y: y + j });
    return out;
}

/** Whether the terrain draws all of a cell at level 18 by the rule, and whether it draws any of it at level 18. */
function levels(rule: Rule, cell: Node): { whole: boolean; some: boolean } {
    if (!rule.split(cell)) return { whole: false, some: false };
    let n = 0;
    for (let q = 0; q < 4; q++) if (rule.split({ level: CELL_LEVEL + 1, x: 2 * cell.x + (q & 1), y: 2 * cell.y + (q >> 1) })) n++;
    return { whole: n === 4, some: n > 0 };
}

/** The cells whose meshes the eye wants: those with a level-18 node, and those that will have one within LEAD_M. */
export function meshCells(rule: Rule, eye: Eye): Set<string> {
    const out = new Set<string>();
    if (rule.finest < 18 || globalThis.location?.search.includes('pieces=off')) return out;
    // A level-17 node splits nearer than this.
    const split = rule.range / 2 ** 18;
    for (const cell of cellsAbout(eye, WINDOW >> 1)) if (distances(eye, cell)[0] < split + LEAD_M) out.add(key(cell));
    return out;
}

export class Pieces {
    private pieces = new Map<string, Piece>();
    private resources?: Resources;
    /** The cells that draw, nearest first, those in view, and those of them that draw only in part at level 18. */
    private drawn: Piece[] = [];
    private inView: Piece[] = [];
    private cut = new Set<Piece>();
    private window?: [number, number];
    private slots = new Map<string, number>();
    private cellsDirty = true;
    private readyFrom?: number;
    private readyMs = 0;

    /** A cell's mesh arrives with the first word of its records block. */
    add(node: Node, bytes: ArrayBuffer, block: number): void {
        this.remove(node);
        this.pieces.set(key(node), { node, mesh: cellMesh(bytes), block, computed: false, stale: true });
    }

    remove(node: Node): void {
        const piece = this.pieces.get(key(node));
        if (!piece) return;
        this.release(piece);
        this.pieces.delete(key(node));
        this.cellsDirty = true;
    }

    private release(piece: Piece): void {
        const r = this.resources;
        if (r && piece.gpu) for (const buffer of [piece.gpu.data, piece.gpu.indices, piece.gpu.params]) r.gpu.destroy('plan', buffer);
        piece.gpu = undefined;
        piece.computed = false;
        piece.stale = true;
    }

    /** Takes the pipelines on a device: `frame` is the frame uniform of the eye's view, `cells` the buffer of CELLS_BYTES, `records` the records' texture. */
    async attach(gpu: Gpu, frame: GPUBuffer, cells: GPUBuffer, records: GPUTexture): Promise<void> {
        this.resources = undefined;
        for (const piece of this.pieces.values()) piece.gpu = undefined, piece.computed = false, piece.stale = true;
        const made = await gpu.once(piecePipelines);
        this.resources = { gpu, frame, cells, records, ...made };
        this.cellsDirty = true;
    }

    /**
     * Picks the cells that draw: those with a level-18 node whose mesh holds its heights. `block` gives each
     * cell's records block, `reshaped` the nodes whose stored pages changed, `visible` whether a cell can show,
     * `settled` whether a cell's mesh came or never comes. Returns whether what draws changed.
     */
    update(rule: Rule, eye: Eye, block: (node: Node) => number | undefined, reshaped: Node[], visible: (node: Node) => boolean, settled: (node: Node) => boolean): boolean {
        for (const piece of this.pieces.values()) {
            const now = block(piece.node) ?? -1;
            if (now !== piece.block) {
                piece.block = now;
                piece.stale = true;
            }
            if (reshaped.some(node => overlaps(node, piece.node))) piece.stale = true;
        }
        const before = this.drawn.map(piece => key(piece.node)).join();
        const cells = rule.finest >= 18 ? cellsAbout(eye, WINDOW >> 1).map(cell => ({ cell, ...levels(rule, cell) })).filter(({ some }) => some) : [];
        const near = (piece: Piece) => distances(eye, piece.node)[0];
        this.cut.clear();
        this.drawn = cells.flatMap(({ cell, whole }) => {
            const piece = this.pieces.get(key(cell));
            if (piece?.computed && !whole) this.cut.add(piece);
            return piece?.computed ? [piece] : [];
        }).sort((a, b) => near(a) - near(b));
        this.inView = this.drawn.filter(piece => visible(piece.node));
        const waiting = cells.some(({ cell }) => !this.pieces.get(key(cell))?.computed && !settled(cell));
        const now = performance.now();
        if (waiting) this.readyFrom ??= now;
        else if (this.readyFrom !== undefined) {
            this.readyMs = now - this.readyFrom;
            this.readyFrom = undefined;
        }
        const changed = before !== this.drawn.map(piece => key(piece.node)).join();
        this.cellsDirty ||= changed;
        return changed || [...this.pieces.values()].some(piece => piece.stale);
    }

    /** Uploads new pieces and the window of the cells that draw. */
    write(eye: Eye): void {
        const r = this.resources;
        if (!r) return;
        const { device } = r.gpu;
        for (const piece of this.pieces.values()) if (!piece.gpu) this.upload(r, piece);
        const s = size(CELL_LEVEL), window: [number, number] = [Math.floor((eye.anchor.whole[0] + eye.anchor.part[0]) / s) - (WINDOW >> 1), Math.floor((eye.anchor.whole[1] + eye.anchor.part[1]) / s) - (WINDOW >> 1)];
        if (!this.cellsDirty && window[0] === this.window?.[0] && window[1] === this.window?.[1]) return;
        this.cellsDirty = false;
        this.window = window;
        const names = new Set(this.drawn.map(piece => key(piece.node)));
        for (const name of this.slots.keys()) if (!names.has(name)) this.slots.delete(name);
        const used = new Set(this.slots.values()), head = new Int32Array(HEAD);
        head.set(window);
        for (const piece of this.drawn) {
            const name = key(piece.node), i = piece.node.x - window[0], j = piece.node.y - window[1];
            if (i < 0 || j < 0 || i >= WINDOW || j >= WINDOW) continue;
            let slot = this.slots.get(name);
            if (slot === undefined) {
                slot = [...Array(SLOTS).keys()].find(k => !used.has(k));
                if (slot === undefined) continue;
                used.add(slot);
                this.slots.set(name, slot);
                device.queue.writeBuffer(r.cells, 4 * (HEAD + slot * SUNK_WORDS), piece.mesh.sunk);
            }
            head[2 + j * WINDOW + i] = slot + 1;
        }
        // A cell without a slot does not draw: its terrain would neither sink nor lose its snap.
        this.drawn = this.drawn.filter(piece => this.slots.has(key(piece.node)));
        this.inView = this.inView.filter(piece => this.slots.has(key(piece.node)));
        device.queue.writeBuffer(r.cells, 0, head);
    }

    private upload(r: Resources, piece: Piece): void {
        const { device } = r.gpu, { corners, triangles, steps } = piece.mesh, n = corners.length / 2;
        const data = r.gpu.buffer('plan', 4 * Math.max(4, 6 * n + steps.length), GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST | GPUBufferUsage.COPY_SRC);
        const indices = r.gpu.buffer('plan', 4 * Math.max(3, triangles.length), GPUBufferUsage.INDEX | GPUBufferUsage.COPY_DST);
        const params = r.gpu.buffer('plan', PARAMS_BYTES, GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST);
        device.queue.writeBuffer(data, 0, corners);
        device.queue.writeBuffer(data, 4 * 6 * n, steps);
        device.queue.writeBuffer(indices, 0, triangles);
        const draw = device.createBindGroup({ layout: r.drawLayout, entries: [
            { binding: 0, resource: { buffer: r.frame } }, { binding: 1, resource: { buffer: data } }, { binding: 2, resource: { buffer: params } }, { binding: 3, resource: { buffer: r.cells } },
        ] });
        const compute = device.createBindGroup({ layout: r.computeLayout, entries: [
            { binding: 0, resource: { buffer: data } }, { binding: 1, resource: { buffer: params } }, { binding: 8, resource: r.records.createView() },
        ] });
        piece.gpu = { data, indices, params, draw, compute };
        piece.computed = false;
        piece.stale = true;
    }

    /** Makes the heights of the stale pieces near the eye first, within CORNERS_PER_FRAME; after the frame's pages and records. */
    encode(encoder: GPUCommandEncoder, surface: GPUBindGroup, eye: Eye): void {
        const r = this.resources;
        if (!r) return;
        const stale = [...this.pieces.values()].filter(piece => piece.stale && piece.gpu && piece.block >= 0).sort((a, b) => distances(eye, a.node)[0] - distances(eye, b.node)[0]);
        if (!stale.length) return;
        const pass = encoder.beginComputePass({ label: 'plan corners' });
        pass.setPipeline(r.corners);
        pass.setBindGroup(1, surface);
        let corners = 0;
        for (const piece of stale) {
            const n = piece.mesh.corners.length / 2, s = size(CELL_LEVEL);
            if (corners && corners + n > CORNERS_PER_FRAME) break;
            corners += n;
            const params = new ArrayBuffer(PARAMS_BYTES), ints = new Int32Array(params), words = new Uint32Array(params);
            ints.set([piece.node.x * s, piece.node.y * s]);
            words.set([n, 6 * n, 2 * n, piece.mesh.steps.length / 4, piece.block, 0], 2);
            r.gpu.device.queue.writeBuffer(piece.gpu!.params, 0, params);
            pass.setBindGroup(0, piece.gpu!.compute);
            pass.dispatchWorkgroups(Math.ceil(n / 64));
            piece.computed = true;
            piece.stale = false;
        }
        pass.end();
    }

    /** Before the terrain, nearest first: early depth then rejects the terrain under the meshes. */
    draw(pass: GPURenderPassEncoder, surface: GPUBindGroup, ground: GPUBindGroup, features: GPUBindGroup): void {
        const r = this.resources;
        if (!r || !this.inView.length) return;
        pass.setBindGroup(1, surface);
        pass.setBindGroup(2, ground);
        pass.setBindGroup(3, features);
        for (const cut of [false, true]) {
            const pieces = this.inView.filter(piece => this.cut.has(piece) === cut);
            if (!pieces.length) continue;
            pass.setPipeline(cut ? r.cutTriangles : r.triangles);
            for (const piece of pieces) {
                pass.setBindGroup(0, piece.gpu!.draw);
                pass.setIndexBuffer(piece.gpu!.indices, 'uint32');
                pass.drawIndexed(piece.mesh.triangles.length);
            }
            pass.setPipeline(cut ? r.cutSlots : r.slots);
            for (const piece of pieces) {
                if (!piece.mesh.steps.length) continue;
                pass.setBindGroup(0, piece.gpu!.draw);
                pass.draw(6 * (piece.mesh.steps.length / 4));
            }
        }
    }

    /** True while a cell that the terrain draws in part at level 18 has no mesh that draws. */
    get loading(): boolean {
        return this.readyFrom !== undefined && !globalThis.location?.search.includes('pieces=off');
    }

    get stats(): PiecesStats {
        const bytes = [...this.pieces.values()].reduce((sum, { mesh }) => sum + 4 * (3 * mesh.corners.length + mesh.steps.length + mesh.triangles.length), 0);
        return {
            cells: this.pieces.size, drawn: this.drawn.length, inView: this.inView.length,
            triangles: this.inView.reduce((sum, { mesh }) => sum + mesh.triangles.length / 3, 0), steps: this.inView.reduce((sum, { mesh }) => sum + mesh.steps.length / 4, 0),
            bytes, ready: this.readyMs,
        };
    }
}

/** Whether two nodes overlap: one holds the other. */
function overlaps(a: Node, b: Node): boolean {
    const [fine, coarse] = a.level >= b.level ? [a, b] : [b, a], shift = fine.level - coarse.level;
    return fine.x >> shift === coarse.x && fine.y >> shift === coarse.y;
}

async function piecePipelines(gpu: Gpu) {
    const { device } = gpu, { surface, ground, features } = gpu.once(layouts);
    const V = GPUShaderStage.VERTEX, C = GPUShaderStage.COMPUTE;
    const drawLayout = device.createBindGroupLayout({ entries: [
        { binding: 0, visibility: V | GPUShaderStage.FRAGMENT, buffer: { type: 'uniform' } }, { binding: 1, visibility: V, buffer: { type: 'read-only-storage' } },
        { binding: 2, visibility: V, buffer: { type: 'uniform' } }, { binding: 3, visibility: V, buffer: { type: 'read-only-storage' } },
    ] });
    const computeLayout = device.createBindGroupLayout({ entries: [
        { binding: 0, visibility: C, buffer: { type: 'storage' } }, { binding: 1, visibility: C, buffer: { type: 'uniform' } }, { binding: 8, visibility: C, texture: { sampleType: 'uint' } },
    ] });
    const module = device.createShaderModule({ label: 'plan pieces', code: wgsl('plan/piece.wgsl') });
    const render = (entryPoint: string, cullMode: GPUCullMode, cut: boolean) => device.createRenderPipelineAsync({
        label: `plan ${entryPoint}${cut ? ' cut' : ''}`, layout: device.createPipelineLayout({ bindGroupLayouts: [drawLayout, surface, ground, features] }),
        vertex: { module, entryPoint },
        // The ground without the road paint, as the corridors' band.
        fragment: { module, entryPoint: 'corridor_fragment', targets: [{ format: sceneFormat(gpu) }], constants: { BAND: 1, PIECE: 1, CUT: cut ? 1 : 0 } },
        primitive: { topology: 'triangle-list', cullMode },
        depthStencil: { format: DEPTH, depthWriteEnabled: true, depthCompare: 'greater' },
    });
    // A step faces its lower side from either side of its edge.
    const [triangles, slots, cutTriangles, cutSlots, corners] = await Promise.all([
        render('piece_vertex', 'back', false), render('slot_vertex', 'none', false), render('piece_vertex', 'back', true), render('slot_vertex', 'none', true),
        device.createComputePipelineAsync({
            label: 'plan corners', layout: device.createPipelineLayout({ bindGroupLayouts: [computeLayout, surface] }),
            compute: { module: device.createShaderModule({ label: 'plan corners', code: wgsl('plan/corners.wgsl') }), entryPoint: 'corners' },
        }),
    ]);
    return { triangles, slots, cutTriangles, cutSlots, corners, drawLayout, computeLayout };
}
