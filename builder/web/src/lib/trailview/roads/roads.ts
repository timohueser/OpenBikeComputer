import type { Gpu } from '../gpu/device';
import { layouts } from '../gpu/layouts';
import { wgsl } from '../gpu/wgsl';
import { CASTER_DEPTH, CASTER_PRIMITIVE } from '../light/shadow';
import { DEPTH, sceneFormat } from '../resolve/targets';
import { featureTile } from '../terrain/bins';
import { CHILD, key, OWNER, size, type Node } from '../terrain/lod';
import type { FeatureTiles } from '../terrain/stamps';
import { Pool } from '../structures/pool';
import { stopped } from '../stream/terrain';
import { meshBytes, type RoadMesh } from './mesh';
import { ROAD_VERTEX, type PieceRange, type RoadPiece } from './piece';

/** Roads draw as corridors this far from the eye; past it the paint draws them. The one value of the range. */
export const RANGE_M = 600;
/** A piece draws its far range from this far from the eye. */
const FAR_FROM_M = 200;
/** An owner builds this far before it comes into range: about 20 s at the speed of a car in a valley. */
const AHEAD_M = 300;
/** The GPU pools, with the middles of the draws under the owner's budget of 48 MiB. */
const VERTEX_BYTES = 27 * 2 ** 20, INDEX_BYTES = 20 * 2 ** 20;
/** Bytes of meshes kept on the CPU: with the stamps' 64 MiB, the 96 MiB of the owner's budget. */
const MESH_BYTES = 32 * 2 ** 20;
/** Requests to the worker in flight at most. */
const REQUESTS = 4;
/** Pieces that a frame draws at most: more than lie within the range, also with their split towns. */
const DRAWS = 256;
const NOTHING: RoadMesh = { pieces: [] }, FAILED: RoadMesh = { pieces: [] };
/** Half the side of a child in world units. */
const HALF = size(CHILD) / 2;

/** What the last frame drew, and what the pools hold back. */
export interface RoadStats { pieces: number; vertices: number; triangles: number; near: number; far: number; dropped: number }

interface Resources {
    device: GPUDevice;
    pipeline: GPURenderPipeline;
    caster: GPURenderPipeline;
    group: GPUBindGroup;
    sunGroup: GPUBindGroup;
    pool: Pool;
    /** Per draw the middle of its piece, an instance. */
    middles: GPUBuffer;
}

/** The metres from a world point to the square of a piece. */
const away = ({ x, y }: RoadPiece, ex: number, ey: number, unit: number) => Math.hypot(Math.max(Math.abs(x - ex) - HALF, 0), Math.max(Math.abs(y - ey) - HALF, 0)) * unit;

/**
 * The corridors of the owners about the eye: their meshes from a worker of their own, the fixed pools, and
 * per frame the ranges of the pieces in view. Near pieces come first in the pools, so far pieces go first
 * under pressure. A piece that does not draw leaves the terrain and its paint in view: nothing else stands there.
 */
export class Roads {
    private resources?: Resources;
    private readonly meshes = new Map<string, RoadMesh>();
    private held = 0;
    private readonly pending = new Set<string>();
    private readonly worker?: Worker;
    private readonly waiting = new Map<number, { resolve: (mesh: RoadMesh) => void; reject: (error: Error) => void }>();
    private next = 0;
    /** The pieces in range, near first, with their metres from the eye. */
    private pieces: { piece: RoadPiece; d: number }[] = [];
    /** The metres from the eye to the nearest owner in range without its mesh, up to the range. */
    private built = 0;
    /** Of the pieces in view, each one's place in `pieces` and its range. */
    private draws: { slot: number; range: PieceRange }[] = [];
    failed = 0;

    /** An abort of `stop` ends the worker. */
    constructor(private readonly tiles: FeatureTiles | undefined, private readonly changed: () => void, stop: AbortSignal) {
        if (!tiles) return;
        const worker = this.worker = new Worker(new URL('./road-worker.ts', import.meta.url), { type: 'module' });
        stop.addEventListener('abort', () => worker.terminate());
        worker.onmessage = ({ data }) => {
            const wait = this.waiting.get(data.id);
            this.waiting.delete(data.id);
            if (data.error) wait?.reject(new Error(data.error));
            else wait?.resolve(data.mesh);
        };
    }

    /** Whether meshes are on their way: each wakes the engine when it arrives. */
    get loading(): boolean {
        return this.pending.size > 0;
    }

    get stats(): RoadStats {
        const draws = this.draws.filter(({ slot }) => this.resources?.pool.at(this.pieces[slot].piece));
        const count = (reach: 'near' | 'far') => draws.filter(({ slot, range }) => range === this.pieces[slot].piece[reach]).length;
        return {
            pieces: draws.length, vertices: draws.reduce((sum, { range }) => sum + range.vertices, 0), triangles: draws.reduce((sum, { range }) => sum + range.count / 3, 0),
            near: count('near'), far: count('far'), dropped: this.resources?.pool.dropped ?? 0,
        };
    }

    async attach(gpu: Gpu, [frame, sunFrame]: GPUBuffer[]): Promise<void> {
        this.resources = undefined;
        const { device } = gpu, { layout, pipeline, caster } = await gpu.once(roadPipelines);
        const group = (buffer: GPUBuffer) => device.createBindGroup({ layout, entries: [{ binding: 0, resource: { buffer } }] });
        this.resources = {
            device, pipeline, caster, group: group(frame), sunGroup: group(sunFrame),
            pool: new Pool(device, gpu.buffer('roads', VERTEX_BYTES, GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST), gpu.buffer('roads', INDEX_BYTES, GPUBufferUsage.INDEX | GPUBufferUsage.COPY_DST), ROAD_VERTEX),
            middles: gpu.buffer('roads', 8 * DRAWS, GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST),
        };
    }

    /**
     * The pieces of a frame about the eye (world units, and metres per unit), and of them those that `shows`
     * keeps in view, and whether meshes may build. True when the pieces in range or the owners that hold
     * them differ from the last frame's: either moves what `write` returns.
     */
    update(eye: { x: number; y: number; unit: number }, shows: (piece: RoadPiece) => boolean, build: boolean): boolean {
        const side = size(OWNER), reach = RANGE_M + AHEAD_M, owners: { node: Node; d: number }[] = [], cells = Math.ceil(reach / eye.unit / side);
        const [cx, cy] = [Math.floor(eye.x / side), Math.floor(eye.y / side)];
        for (let y = cy - cells; y <= cy + cells; y++) {
            for (let x = cx - cells; x <= cx + cells; x++) {
                const d = Math.hypot(Math.max(x * side - eye.x, 0, eye.x - (x + 1) * side), Math.max(y * side - eye.y, 0, eye.y - (y + 1) * side)) * eye.unit;
                if (d < reach) owners.push({ node: { level: OWNER, x, y }, d });
            }
        }
        const pieces: { piece: RoadPiece; d: number }[] = [], wanted = new Set<string>();
        let held = 0, built = RANGE_M;
        for (const { node, d } of owners.sort((a, b) => a.d - b.d)) {
            // The farthest owners give way when the nearer ones fill the cache.
            const name = key(node), mesh = held < MESH_BYTES ? this.mesh(node, name, build) : undefined;
            if (mesh) wanted.add(name);
            if (!mesh || mesh === FAILED) built = Math.min(built, d);
            if (!mesh) continue;
            held += meshBytes(mesh);
            for (const piece of mesh.pieces) {
                const d = away(piece, eye.x, eye.y, eye.unit);
                if (d < RANGE_M) pieces.push({ piece, d });
            }
        }
        this.trim(wanted);
        pieces.sort((a, b) => a.d - b.d);
        const changed = built !== this.built || pieces.length !== this.pieces.length || pieces.some(({ piece }, i) => piece !== this.pieces[i].piece);
        this.built = built;
        this.pieces = pieces;
        this.draws = pieces.slice(0, DRAWS).flatMap(({ piece, d }, slot) => (shows(piece) ? [{ slot, range: d < FAR_FROM_M ? piece.near : piece.far }] : []));
        return changed;
    }

    /** Uploads the pieces that the pool lacks and the middles of the frame's draws. Returns the metres from the eye that corridors hold every way to. */
    write(): number {
        const r = this.resources;
        if (!r) return 0;
        r.pool.write(this.pieces.map(({ piece }) => piece));
        const middles = new Int32Array(2 * Math.min(DRAWS, this.pieces.length));
        for (let slot = 0; 2 * slot < middles.length; slot++) middles.set([this.pieces[slot].piece.x, this.pieces[slot].piece.y], 2 * slot);
        if (middles.length) r.device.queue.writeBuffer(r.middles, 0, middles);
        return Math.min(this.built, ...this.pieces.filter(({ piece }) => !r.pool.at(piece)).map(({ d }) => d));
    }

    /** Before the terrain: early depth then rejects the terrain under a road. */
    draw(pass: GPURenderPassEncoder, surface: GPUBindGroup, ground: GPUBindGroup, features: GPUBindGroup): void {
        const r = this.resources;
        if (!r || !this.draws.length) return;
        [r.group, surface, ground, features].forEach((group, k) => pass.setBindGroup(k, group));
        pass.setPipeline(r.pipeline);
        this.drawRanges(pass, r, this.draws);
    }

    /** Draws the faces that cast into the shadow map (light/shadow.ts). */
    cast(pass: GPURenderPassEncoder, surface: GPUBindGroup): void {
        const r = this.resources, casters = this.pieces.slice(0, DRAWS).flatMap(({ piece }, slot) => (piece.casters.count ? [{ slot, range: piece.casters }] : []));
        if (!r || !casters.length) return;
        pass.setBindGroup(0, r.sunGroup);
        pass.setBindGroup(1, surface);
        pass.setPipeline(r.caster);
        this.drawRanges(pass, r, casters);
    }

    private drawRanges(pass: GPURenderPassEncoder, r: Resources, draws: { slot: number; range: PieceRange }[]): void {
        pass.setVertexBuffer(0, r.pool.vertices);
        pass.setVertexBuffer(1, r.middles);
        pass.setIndexBuffer(r.pool.indices, 'uint16');
        // The first instance names the middle of the piece.
        for (const { slot, range } of draws) {
            const at = r.pool.at(this.pieces[slot].piece);
            if (at) pass.drawIndexed(range.count, 1, at.first + range.first, at.base, slot);
        }
    }

    /** An owner's mesh, undefined while it is on its way. */
    private mesh(node: Node, name: string, build: boolean): RoadMesh | undefined {
        const known = this.meshes.get(name), tile = featureTile(node);
        if (!this.tiles || !tile) return NOTHING;
        if (known) {
            this.meshes.delete(name);
            this.meshes.set(name, known);
            return known;
        }
        if (build && !this.pending.has(name) && this.pending.size < REQUESTS) {
            this.pending.add(name);
            this.ask(tile, node).then(mesh => this.keep(name, mesh), error => {
                // An owner that fails has no corridors, and stops what `write` returns: the paint draws its roads.
                if (!stopped(error)) console.warn(`Road mesh ${name}: ${error instanceof Error ? error.message : error}`);
                this.failed++;
                this.keep(name, FAILED);
            }).finally(() => {
                this.pending.delete(name);
                this.changed();
            });
        }
        return undefined;
    }

    private keep(name: string, mesh: RoadMesh): void {
        this.held += meshBytes(mesh) - (this.meshes.has(name) ? meshBytes(this.meshes.get(name)!) : 0);
        this.meshes.set(name, mesh);
    }

    /** Past the budget, the meshes that have gone longest without use go, but none that the frame wants. */
    private trim(wanted: Set<string>): void {
        for (const [name, mesh] of this.meshes) {
            if (this.held <= MESH_BYTES) break;
            if (wanted.has(name)) continue;
            this.meshes.delete(name);
            this.held -= meshBytes(mesh);
            for (const piece of mesh.pieces) this.resources?.pool.forget(piece);
        }
    }

    private async ask(tile: Node, owner: Node): Promise<RoadMesh> {
        const body = await this.tiles!(tile.level, tile.x, tile.y);
        if (!body) return NOTHING;
        const id = this.next++;
        return new Promise<RoadMesh>((resolve, reject) => {
            this.waiting.set(id, { resolve, reject });
            this.worker!.postMessage({ id, tile, owner, body }, [body]);
        });
    }
}

/** The draw of the corridors, the draw of their casters, and the group of the frame. */
async function roadPipelines(gpu: Gpu) {
    const { device } = gpu, { surface, ground, features } = gpu.once(layouts), module = device.createShaderModule({ label: 'corridors', code: wgsl('roads/corridor.wgsl') });
    const layout = device.createBindGroupLayout({ entries: [{ binding: 0, visibility: GPUShaderStage.VERTEX | GPUShaderStage.FRAGMENT, buffer: { type: 'uniform' } }] });
    const buffers: GPUVertexBufferLayout[] = [{
        arrayStride: ROAD_VERTEX, attributes: [
            { shaderLocation: 0, offset: 0, format: 'sint16x2' }, { shaderLocation: 1, offset: 4, format: 'float32' },
            { shaderLocation: 2, offset: 8, format: 'uint32' }, { shaderLocation: 3, offset: 12, format: 'uint32' },
        ],
    }, { arrayStride: 8, stepMode: 'instance', attributes: [{ shaderLocation: 4, offset: 0, format: 'sint32x2' }] }];
    const [pipeline, caster] = await Promise.all([
        device.createRenderPipelineAsync({
            label: 'corridors', layout: device.createPipelineLayout({ bindGroupLayouts: [layout, surface, ground, features] }), vertex: { module, entryPoint: 'corridor_vertex', buffers },
            // The band shades as the ground without its paint.
            fragment: { module, entryPoint: 'corridor_fragment', targets: [{ format: sceneFormat(gpu) }], constants: { BAND: 1 } },
            primitive: { topology: 'triangle-list', cullMode: 'back' }, depthStencil: { format: DEPTH, depthWriteEnabled: true, depthCompare: 'greater' },
        }),
        device.createRenderPipelineAsync({
            label: 'corridor casters', layout: device.createPipelineLayout({ bindGroupLayouts: [layout, surface] }), vertex: { module, entryPoint: 'corridor_caster', buffers }, primitive: CASTER_PRIMITIVE, depthStencil: CASTER_DEPTH,
        }),
    ]);
    return { layout, pipeline, caster };
}
