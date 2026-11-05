// The ground plan's near mesh with `plan=mesh`: the mesh of each cell about the eye, lit by the
// corridors' fragment stage before the terrain (mesh.wgsl), and the floors that keep the terrain under it
// (floors.wgsl). The cells come built: `debug mesh --export` of host/obc-ground-plan writes them, and the
// page reads them beside itself at MESH_PATH.
import type { Gpu } from '../gpu/device';
import { layouts } from '../gpu/layouts';
import { wgsl } from '../gpu/wgsl';
import { DEPTH, sceneFormat } from '../resolve/targets';
import { stopped } from '../stream/terrain';
import { key, size, type Node } from '../terrain/lod';
import { CELL_LEVEL, wantedCells, type PlanEye } from './plan';

/** Where the page reads a cell `16-x-y.bin`. */
export const MESH_PATH = 'plan-mesh/';
const MAGIC = 0x314d5047;
/** Bytes of a vertex: x, y (int32, frame units from the cell's origin), z (float32), the look word, the place word, the octahedral normal (snorm16 × 2). */
const VERTEX = 24;
/** The floors of a cell: its level-18 grid points, both borders included. As floors.wgsl. */
export const FLOOR_SIDE = 257;
/** The window of cells about the eye's cell whose floors the terrain holds, and the words before the floors. As floors.wgsl. */
export const WINDOW = 7;
const FLOOR_HEAD = 64;
const FLOOR_WORDS = FLOOR_SIDE * FLOOR_SIDE;
/** The bytes of the floors buffer: the head and a cell per place of the window. */
export const FLOOR_BYTES = 4 * (FLOOR_HEAD + WINDOW * WINDOW * FLOOR_WORDS);
/** Cells that load at once. */
const REQUESTS = 4;

/** A cell's mesh: VERTEX bytes per vertex, triangle indices, and the floors in metres (1e30 for none). */
export interface CellMesh { vertices: ArrayBuffer; indices: Uint32Array<ArrayBuffer>; floors: Float32Array<ArrayBuffer> }

/** A cell's mesh from its bytes (`drawn.rs` of the debug tool). */
export function cellMesh(bytes: ArrayBuffer): CellMesh {
    const [magic, vertices, indices, side] = new Uint32Array(bytes, 0, 4);
    if (magic !== MAGIC || side !== FLOOR_SIDE) throw new Error('not a plan mesh');
    const start = 16 + vertices * VERTEX;
    return {
        vertices: bytes.slice(16, start),
        indices: new Uint32Array(bytes.slice(start, start + 4 * indices)),
        floors: new Float32Array(bytes.slice(start + 4 * indices, start + 4 * indices + 4 * side * side)),
    };
}

interface Drawn { node: Node; mesh: CellMesh; vertices?: GPUBuffer; indices?: GPUBuffer }

interface Resources { gpu: Gpu; pipeline: GPURenderPipeline; group: GPUBindGroup; origins: GPUBuffer; floors: GPUBuffer }

export class PlanMesh {
    private resources?: Resources;
    private readonly cells = new Map<string, Drawn | 'none'>();
    private readonly pending = new Set<string>();
    /** The window's north-west cell, and the cells that the last frame drew, by slot. */
    private corner?: [number, number];
    private slots: (Drawn | undefined)[] = [];
    private dirty = true;
    private missing = false;
    failed = 0;

    constructor(private readonly bounds: [number, number, number, number], private readonly changed: () => void, private readonly stop: AbortSignal) {}

    /** True while a cell in range is on its way. */
    get loading(): boolean {
        return this.missing;
    }

    get stats(): { cells: number; triangles: number; failed: number } {
        const drawn = this.slots.filter(cell => cell !== undefined);
        return { cells: drawn.length, triangles: drawn.reduce((sum, { mesh }) => sum + mesh.indices.length / 3, 0), failed: this.failed };
    }

    /** `frame` is the frame uniform of the eye's view; `floors` the terrain's buffer of FLOOR_BYTES. */
    async attach(gpu: Gpu, frame: GPUBuffer, floors: GPUBuffer): Promise<void> {
        this.resources = undefined;
        const { layout, pipeline } = await gpu.once(meshPipeline);
        const group = gpu.device.createBindGroup({ layout, entries: [{ binding: 0, resource: { buffer: frame } }] });
        const origins = gpu.buffer('plan', 8 * WINDOW * WINDOW, GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST);
        for (const cell of this.cells.values()) if (cell !== 'none') cell.vertices = cell.indices = undefined;
        this.resources = { gpu, pipeline, group, origins, floors };
        this.dirty = true;
    }

    /** Loads the cells about the eye and writes the window, or with `on` false clears it; true when what draws changed. */
    update(eye: PlanEye, ahead: [number, number] | undefined, on: boolean): boolean {
        const r = this.resources;
        if (!on) {
            this.missing = false;
            if (!r || !this.corner) return false;
            this.corner = undefined;
            this.slots = [];
            r.gpu.device.queue.writeBuffer(r.floors, 0, new Uint32Array(FLOOR_HEAD));
            return true;
        }
        const wanted = wantedCells(eye, ahead, this.bounds), s = size(CELL_LEVEL);
        const corner: [number, number] = [Math.floor(eye.x / s) - (WINDOW >> 1), Math.floor(eye.y / s) - (WINDOW >> 1)];
        const inWindow = ({ x, y }: Node) => x >= corner[0] && x < corner[0] + WINDOW && y >= corner[1] && y < corner[1] + WINDOW;
        const names = new Set(wanted.map(key));
        for (const name of this.cells.keys()) if (!names.has(name)) this.drop(name);
        this.missing = false;
        for (const node of wanted.filter(inWindow)) {
            const name = key(node);
            if (this.cells.has(name)) continue;
            this.missing = true;
            if (this.pending.size < REQUESTS && !this.pending.has(name)) void this.load(node);
        }
        if (corner[0] !== this.corner?.[0] || corner[1] !== this.corner?.[1]) {
            this.corner = corner;
            this.dirty = true;
        }
        if (!r || !this.dirty) return false;
        this.dirty = false;
        this.write(r, corner);
        return true;
    }

    private async load(node: Node): Promise<void> {
        const name = key(node);
        this.pending.add(name);
        try {
            const response = await fetch(new URL(`${MESH_PATH}${node.level}-${node.x}-${node.y}.bin`, document.baseURI), { signal: this.stop });
            this.cells.set(name, response.ok ? { node, mesh: cellMesh(await response.arrayBuffer()) } : 'none');
        } catch (error) {
            if (stopped(error)) return;
            console.warn(`Plan mesh ${name}: ${error instanceof Error ? error.message : error}`);
            this.failed++;
            this.cells.set(name, 'none');
        } finally {
            this.pending.delete(name);
        }
        this.dirty = true;
        this.changed();
    }

    private drop(name: string): void {
        const cell = this.cells.get(name);
        this.cells.delete(name);
        if (!cell || cell === 'none' || !this.resources) return;
        for (const buffer of [cell.vertices, cell.indices]) if (buffer) this.resources.gpu.destroy('plan', buffer);
        this.dirty = true;
    }

    /** The window's cells: their buffers, origins and floors. */
    private write(r: Resources, corner: [number, number]): void {
        const { device } = r.gpu, s = size(CELL_LEVEL);
        const head = new Uint32Array(FLOOR_HEAD), origins = new Int32Array(2 * WINDOW * WINDOW);
        head.set(new Uint32Array(new Int32Array(corner).buffer));
        this.slots = [];
        for (let k = 0; k < WINDOW * WINDOW; k++) {
            const x = corner[0] + (k % WINDOW), y = corner[1] + Math.floor(k / WINDOW);
            const cell = this.cells.get(key({ level: CELL_LEVEL, x, y }));
            origins.set([x * s, y * s], 2 * k);
            if (!cell || cell === 'none' || !cell.mesh.indices.length) continue;
            if (!cell.vertices) {
                cell.vertices = r.gpu.buffer('plan', cell.mesh.vertices.byteLength, GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST);
                cell.indices = r.gpu.buffer('plan', cell.mesh.indices.byteLength, GPUBufferUsage.INDEX | GPUBufferUsage.COPY_DST);
                device.queue.writeBuffer(cell.vertices, 0, cell.mesh.vertices);
                device.queue.writeBuffer(cell.indices, 0, cell.mesh.indices);
            }
            device.queue.writeBuffer(r.floors, 4 * (FLOOR_HEAD + k * FLOOR_WORDS), cell.mesh.floors);
            head[2 + k] = 1;
            this.slots[k] = cell;
        }
        device.queue.writeBuffer(r.floors, 0, head);
        device.queue.writeBuffer(r.origins, 0, origins);
    }

    /** Before the terrain: early depth then rejects the terrain under the mesh. */
    draw(pass: GPURenderPassEncoder, surface: GPUBindGroup, ground: GPUBindGroup, features: GPUBindGroup): void {
        const r = this.resources;
        if (!r) return;
        [r.group, surface, ground, features].forEach((group, k) => pass.setBindGroup(k, group));
        pass.setPipeline(r.pipeline);
        pass.setVertexBuffer(1, r.origins);
        this.slots.forEach((cell, slot) => {
            if (!cell?.vertices || !cell.indices) return;
            pass.setVertexBuffer(0, cell.vertices);
            pass.setIndexBuffer(cell.indices, 'uint32');
            pass.drawIndexed(cell.mesh.indices.length, 1, 0, 0, slot);
        });
    }
}

async function meshPipeline(gpu: Gpu) {
    const { device } = gpu, { surface, ground, features } = gpu.once(layouts), module = device.createShaderModule({ label: 'plan mesh', code: wgsl('plan/mesh.wgsl') });
    const layout = device.createBindGroupLayout({ entries: [{ binding: 0, visibility: GPUShaderStage.VERTEX | GPUShaderStage.FRAGMENT, buffer: { type: 'uniform' } }] });
    const pipeline = await device.createRenderPipelineAsync({
        label: 'plan mesh', layout: device.createPipelineLayout({ bindGroupLayouts: [layout, surface, ground, features] }),
        vertex: {
            module, entryPoint: 'plan_vertex', buffers: [{
                arrayStride: VERTEX, attributes: [
                    { shaderLocation: 0, offset: 0, format: 'sint32x2' }, { shaderLocation: 1, offset: 8, format: 'float32' }, { shaderLocation: 2, offset: 12, format: 'uint32' },
                    { shaderLocation: 3, offset: 16, format: 'uint32' }, { shaderLocation: 4, offset: 20, format: 'snorm16x2' },
                ],
            }, { arrayStride: 8, stepMode: 'instance', attributes: [{ shaderLocation: 5, offset: 0, format: 'sint32x2' }] }],
        },
        // The ground without the road paint, as the corridors' band.
        fragment: { module, entryPoint: 'corridor_fragment', targets: [{ format: sceneFormat(gpu) }], constants: { BAND: 1, PIECE: 1 } },
        primitive: { topology: 'triangle-list', cullMode: 'back' },
        depthStencil: { format: DEPTH, depthWriteEnabled: true, depthCompare: 'greater' },
    });
    return { layout, pipeline };
}
