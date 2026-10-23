import type { Gpu } from '../gpu/device';
import { wgsl } from '../gpu/wgsl';
import { metresPerUnit } from '../terrain/bins';
import { key, type Node } from '../terrain/lod';
import type { Stamps } from '../terrain/stamps';
import { STRUCTURE_VERTEX, type StructureMesh } from './mesh';
import { FLOOR, OWNER } from './node';

/**
 * The structure meshes on the GPU, a fixed pool. A new mesh goes after the others; when one does not
 * fit and the pool holds meshes that the frame does not draw, the pool starts again with the meshes of
 * the frame.
 */
const VERTEX_BYTES = 8 * 2 ** 20, INDEX_BYTES = 4 * 2 ** 20;
/**
 * A floor reaches this far past the ends of its segments: off a deck or out of a tunnel the ground holds
 * the road's height, and the probe of the ground under the eye, a few frames late, catches up meanwhile.
 */
const REACH_M = 3;

interface Resources {
    device: GPUDevice;
    pipeline: GPURenderPipeline;
    group: GPUBindGroup;
    vertices: GPUBuffer;
    indices: GPUBuffer;
}

/**
 * The structures, with structures.wgsl: the mesh of each OWNER node that a drawn node of that level
 * or finer lies in, once. They draw before the terrain, so the terrain behind them shades nothing.
 */
export class Structures {
    private resources?: Resources;
    private draws: StructureMesh[] = [];
    private placed = new Map<StructureMesh, { first: number; base: number }>();
    private vertexEnd = 0;
    private indexEnd = 0;
    /** The meshes of the last frame that did not fit in the pool. */
    dropped = 0;
    private drawn = '';

    constructor(private readonly stamps: Stamps) {}

    async attach(gpu: Gpu, frame: GPUBuffer, surfaceLayout: GPUBindGroupLayout, groundLayout: GPUBindGroupLayout, featureLayout: GPUBindGroupLayout, format: GPUTextureFormat, depthFormat: GPUTextureFormat): Promise<void> {
        const { device } = gpu;
        const layout = device.createBindGroupLayout({ entries: [{ binding: 0, visibility: GPUShaderStage.VERTEX | GPUShaderStage.FRAGMENT, buffer: { type: 'uniform' } }] });
        const module = device.createShaderModule({ label: 'structures', code: wgsl('structures/structures.wgsl') });
        const pipeline = await device.createRenderPipelineAsync({
            label: 'structures', layout: device.createPipelineLayout({ bindGroupLayouts: [layout, surfaceLayout, groundLayout, featureLayout] }),
            vertex: {
                module, entryPoint: 'structure_vertex', buffers: [{
                    arrayStride: STRUCTURE_VERTEX * 4, attributes: [
                        { shaderLocation: 0, offset: 0, format: 'sint32x2' }, { shaderLocation: 1, offset: 8, format: 'float32' },
                        { shaderLocation: 2, offset: 12, format: 'unorm8x4' }, { shaderLocation: 3, offset: 16, format: 'float32x2' },
                    ],
                }],
            },
            fragment: { module, entryPoint: 'structure_fragment', targets: [{ format }] },
            primitive: { topology: 'triangle-list', cullMode: 'back' },
            depthStencil: { format: depthFormat, depthWriteEnabled: true, depthCompare: 'greater' },
        });
        this.placed.clear();
        this.vertexEnd = this.indexEnd = 0;
        this.resources = {
            device, pipeline,
            group: device.createBindGroup({ layout, entries: [{ binding: 0, resource: { buffer: frame } }] }),
            vertices: gpu.buffer('structures', VERTEX_BYTES, GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST),
            indices: gpu.buffer('structures', INDEX_BYTES, GPUBufferUsage.INDEX | GPUBufferUsage.COPY_DST),
        };
    }

    /** The meshes of a frame from its drawn nodes, which ask for them. True when they differ from the last frame's. */
    update(drawn: Node[]): boolean {
        const owners = new Map<string, Node>();
        for (const { level, x, y } of drawn) {
            if (level < OWNER) continue;
            const owner = { level: OWNER, x: x >> (level - OWNER), y: y >> (level - OWNER) };
            owners.set(key(owner), owner);
        }
        const names: string[] = [];
        this.draws = [];
        for (const [name, owner] of owners) {
            const mesh = this.stamps.get(owner)?.structures;
            if (!mesh) continue;
            this.draws.push(mesh);
            names.push(name);
        }
        const now = names.join(' ');
        if (now === this.drawn) return false;
        this.drawn = now;
        return true;
    }

    /**
     * The highest floor of a deck or a tunnel at world point x, y between heights `low` and `high`, where
     * its node's floors are known.
     */
    floor(x: number, y: number, low: number, high: number): number | undefined {
        const side = 2 ** (30 - OWNER), floors = this.stamps.known({ level: OWNER, x: Math.floor(x / side), y: Math.floor(y / side) })?.floors;
        if (!floors) return undefined;
        let best: number | undefined;
        for (let i = 0; i < floors.length; i += FLOOR) {
            const [ax, ay, bx, by, za, zb, half] = floors.subarray(i, i + FLOOR), dx = bx - ax, dy = by - ay, l2 = dx * dx + dy * dy || 1;
            const t = ((x - ax) * dx + (y - ay) * dy) / l2, past = REACH_M / metresPerUnit(y) / Math.sqrt(l2);
            if (t < -past || t > 1 + past || Math.abs((x - ax) * dy - (y - ay) * dx) / Math.sqrt(l2) * metresPerUnit(y) > half) continue;
            const z = za + (zb - za) * t;
            if (z >= low && z <= high && (best === undefined || z > best)) best = z;
        }
        return best;
    }

    /** Uploads the meshes that the pool lacks. */
    write(): void {
        const r = this.resources;
        if (!r) return;
        const placeAll = () => {
            for (const mesh of this.draws) if (!this.placed.has(mesh)) this.place(r, mesh);
        };
        const missing = () => this.draws.filter(mesh => !this.placed.has(mesh)).length;
        placeAll();
        const used = new Set(this.draws);
        if (missing() && [...this.placed.keys()].some(mesh => !used.has(mesh))) {
            this.placed.clear();
            this.vertexEnd = this.indexEnd = 0;
            placeAll();
        }
        this.dropped = missing();
    }

    private place(r: Resources, mesh: StructureMesh): void {
        if (this.vertexEnd + mesh.vertices.byteLength > VERTEX_BYTES || this.indexEnd + mesh.indices.byteLength > INDEX_BYTES) return;
        r.device.queue.writeBuffer(r.vertices, this.vertexEnd, mesh.vertices);
        r.device.queue.writeBuffer(r.indices, this.indexEnd, mesh.indices);
        this.placed.set(mesh, { first: this.indexEnd / 4, base: this.vertexEnd / (STRUCTURE_VERTEX * 4) });
        this.vertexEnd += mesh.vertices.byteLength;
        this.indexEnd += mesh.indices.byteLength;
    }

    draw(pass: GPURenderPassEncoder, surfaceGroup: GPUBindGroup, ground: GPUBindGroup, features: GPUBindGroup): void {
        const r = this.resources;
        if (!r || !this.draws.length) return;
        pass.setPipeline(r.pipeline);
        pass.setBindGroup(0, r.group);
        pass.setBindGroup(1, surfaceGroup);
        pass.setBindGroup(2, ground);
        pass.setBindGroup(3, features);
        pass.setVertexBuffer(0, r.vertices);
        pass.setIndexBuffer(r.indices, 'uint32');
        for (const mesh of this.draws) {
            const at = this.placed.get(mesh);
            if (at) pass.drawIndexed(mesh.indices.length, 1, at.first, at.base);
        }
    }
}
