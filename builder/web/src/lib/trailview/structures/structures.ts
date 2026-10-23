import type { Gpu } from '../gpu/device';
import { wgsl } from '../gpu/wgsl';
import { COARSEST, key, type Node } from '../terrain/lod';
import type { Stamps } from '../terrain/stamps';
import { STRUCTURE_VERTEX, type StructureMesh } from './mesh';

/**
 * The structure meshes on the GPU, a fixed pool. A new mesh goes after the others; when one does not
 * fit and the pool holds meshes that the frame does not draw, the pool starts again with the meshes of
 * the frame, finest first.
 */
const VERTEX_BYTES = 8 * 2 ** 20, INDEX_BYTES = 4 * 2 ** 20;

/** A mesh, the level of the node that it belongs to, and the drawn nodes that draw it. */
interface Draw { mesh: StructureMesh; level: number; nodes: number[] }

interface Resources {
    device: GPUDevice;
    pipeline: GPURenderPipeline;
    group: GPUBindGroup;
    vertices: GPUBuffer;
    indices: GPUBuffer;
    instances: GPUBuffer;
}

/**
 * The structures of the drawn nodes, with structures.wgsl: each draws the mesh of the node whose page
 * it draws, or of that node's nearest ancestor whose mesh is known.
 */
export class Structures {
    private resources?: Resources;
    private draws: Draw[] = [];
    private placed = new Map<StructureMesh, { first: number; base: number }>();
    private vertexEnd = 0;
    private indexEnd = 0;
    /** The meshes of the last frame that did not fit in the pool. */
    dropped = 0;
    private drawn = '';

    constructor(private readonly stamps: Stamps) {}

    async attach(gpu: Gpu, frame: GPUBuffer, nodes: GPUBuffer, surfaceLayout: GPUBindGroupLayout, groundLayout: GPUBindGroupLayout, featureLayout: GPUBindGroupLayout, format: GPUTextureFormat, depthFormat: GPUTextureFormat): Promise<void> {
        const { device } = gpu, V = GPUShaderStage.VERTEX;
        const layout = device.createBindGroupLayout({ entries: [
            { binding: 0, visibility: V | GPUShaderStage.FRAGMENT, buffer: { type: 'uniform' } },
            { binding: 2, visibility: V, buffer: { type: 'read-only-storage' } },
            { binding: 3, visibility: V, buffer: { type: 'read-only-storage' } },
        ] });
        const instances = gpu.buffer('structures', nodes.size / 2, GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST);
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
            device, pipeline, instances,
            group: device.createBindGroup({ layout, entries: [{ binding: 0, resource: { buffer: frame } }, { binding: 2, resource: { buffer: nodes } }, { binding: 3, resource: { buffer: instances } }] }),
            vertices: gpu.buffer('structures', VERTEX_BYTES, GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST),
            indices: gpu.buffer('structures', INDEX_BYTES, GPUBufferUsage.INDEX | GPUBufferUsage.COPY_DST),
        };
    }

    /**
     * The draws of a frame: per mesh, the drawn nodes that draw it. `drawing` has the node of the page
     * that each drawn node draws. True when they differ from the last frame's.
     */
    update(drawing: Node[]): boolean {
        const draws = new Map<string, Draw>();
        drawing.forEach((node, i) => {
            for (let { level, x, y } = node; level >= COARSEST; level--, x >>= 1, y >>= 1) {
                const known = this.stamps.known({ level, x, y });
                if (!known) continue;
                if (!known.structures) return;
                const name = key({ level, x, y }), draw = draws.get(name) ?? { mesh: known.structures, level, nodes: [] };
                draws.set(name, draw);
                draw.nodes.push(i);
                return;
            }
        });
        this.draws = [...draws.values()].sort((a, b) => b.level - a.level);
        const drawn = [...draws].map(([name, { nodes }]) => `${name}:${nodes}`).join(' ');
        if (drawn === this.drawn) return false;
        this.drawn = drawn;
        return true;
    }

    /** Uploads the meshes that the pool lacks, and the instances of each draw. */
    write(): void {
        const r = this.resources;
        if (!r) return;
        const placeAll = () => {
            for (const { mesh } of this.draws) if (!this.placed.has(mesh)) this.place(r, mesh);
        };
        const missing = () => this.draws.filter(({ mesh }) => !this.placed.has(mesh)).length;
        placeAll();
        const used = new Set(this.draws.map(({ mesh }) => mesh));
        if (missing() && [...this.placed.keys()].some(mesh => !used.has(mesh))) {
            this.placed.clear();
            this.vertexEnd = this.indexEnd = 0;
            placeAll();
        }
        this.dropped = missing();
        const instances = this.draws.flatMap(({ mesh, level, nodes }) => (this.placed.has(mesh) ? nodes.flatMap(node => [node, level]) : []));
        if (instances.length) r.device.queue.writeBuffer(r.instances, 0, new Uint32Array(instances));
    }

    private place(r: Resources, mesh: StructureMesh): boolean {
        if (this.vertexEnd + mesh.vertices.byteLength > VERTEX_BYTES || this.indexEnd + mesh.indices.byteLength > INDEX_BYTES) return false;
        r.device.queue.writeBuffer(r.vertices, this.vertexEnd, mesh.vertices);
        r.device.queue.writeBuffer(r.indices, this.indexEnd, mesh.indices);
        this.placed.set(mesh, { first: this.indexEnd / 4, base: this.vertexEnd / (STRUCTURE_VERTEX * 4) });
        this.vertexEnd += mesh.vertices.byteLength;
        this.indexEnd += mesh.indices.byteLength;
        return true;
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
        let instance = 0;
        for (const { mesh, nodes } of this.draws) {
            const at = this.placed.get(mesh);
            if (!at) continue;
            pass.drawIndexed(mesh.indices.length, nodes.length, at.first, at.base, instance);
            instance += nodes.length;
        }
    }
}
