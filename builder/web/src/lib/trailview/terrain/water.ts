import type { Gpu } from '../gpu/device';
import { wgsl } from '../gpu/wgsl';
import { featureTile } from './bins';
import { key, type Node } from './lod';
import type { Stamps } from './stamps';
import { WATER_VERTEX, type WaterMesh } from './water-mesh';

/** The water meshes on the GPU, a fixed pool: the tiles of the first drawn nodes first. */
const VERTEX_BYTES = 8 * 2 ** 20, INDEX_BYTES = 8 * 2 ** 20;

interface Draw { name: string; mesh: WaterMesh; nodes: number[] }

interface Resources {
    device: GPUDevice;
    pipeline: GPURenderPipeline;
    group: GPUBindGroup;
    vertices: GPUBuffer;
    indices: GPUBuffer;
    instances: GPUBuffer;
}

/**
 * The water of the drawn nodes: each draws the water mesh of the feature tile that its stamps read,
 * from the stamp worker, with water.wgsl.
 */
export class Water {
    private resources?: Resources;
    private draws: Draw[] = [];
    /** Per draw in the buffers: its first index, its first vertex and its index count; and the tiles that the buffers were filled for. */
    private placed = new Map<string, { first: number; base: number; count: number }>();
    private filled = '';

    constructor(private readonly stamps: Stamps) {}

    async attach(gpu: Gpu, frame: GPUBuffer, nodes: GPUBuffer, surfaceLayout: GPUBindGroupLayout, depthFormat: GPUTextureFormat): Promise<void> {
        const { device } = gpu, V = GPUShaderStage.VERTEX;
        const layout = device.createBindGroupLayout({ entries: [
            { binding: 0, visibility: V | GPUShaderStage.FRAGMENT, buffer: { type: 'uniform' } },
            { binding: 2, visibility: V, buffer: { type: 'read-only-storage' } },
            { binding: 3, visibility: V, buffer: { type: 'read-only-storage' } },
        ] });
        const instances = gpu.buffer('water', nodes.size / 4, GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST);
        const module = device.createShaderModule({ label: 'water', code: wgsl('terrain/water.wgsl') });
        const pipeline = await device.createRenderPipelineAsync({
            label: 'water', layout: device.createPipelineLayout({ bindGroupLayouts: [layout, surfaceLayout] }),
            vertex: {
                module, entryPoint: 'water_vertex', buffers: [{
                    arrayStride: WATER_VERTEX * 4, attributes: [
                        { shaderLocation: 0, offset: 0, format: 'sint32x2' }, { shaderLocation: 1, offset: 8, format: 'float32' }, { shaderLocation: 2, offset: 12, format: 'float32x2' },
                        { shaderLocation: 3, offset: 20, format: 'float32' }, { shaderLocation: 4, offset: 24, format: 'float32' }, { shaderLocation: 5, offset: 28, format: 'float32' },
                    ],
                }],
            },
            fragment: { module, entryPoint: 'water_fragment', targets: [{ format: gpu.format }] },
            primitive: { topology: 'triangle-list' },
            depthStencil: { format: depthFormat, depthWriteEnabled: false, depthCompare: 'greater' },
        });
        this.placed.clear();
        this.filled = '';
        this.resources = {
            device, pipeline, instances,
            group: device.createBindGroup({ layout, entries: [{ binding: 0, resource: { buffer: frame } }, { binding: 2, resource: { buffer: nodes } }, { binding: 3, resource: { buffer: instances } }] }),
            vertices: gpu.buffer('water', VERTEX_BYTES, GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST),
            indices: gpu.buffer('water', INDEX_BYTES, GPUBufferUsage.INDEX | GPUBufferUsage.COPY_DST),
        };
    }

    /** The draws of a frame: per feature tile with water, the drawn nodes that read it. */
    update(nodes: Node[]): void {
        const tiles = new Map<string, Draw | null>();
        nodes.forEach((node, i) => {
            const tile = featureTile(node), name = tile && key(tile);
            if (!tile || !name) return;
            if (!tiles.has(name)) {
                const mesh = this.stamps.mesh(tile);
                tiles.set(name, mesh ? { name, mesh, nodes: [] } : null);
            }
            tiles.get(name)?.nodes.push(i);
        });
        this.draws = [...tiles.values()].filter((draw): draw is Draw => draw !== null);
    }

    /** Uploads the meshes when the frame's tiles change, and the nodes of each draw. */
    write(): void {
        const r = this.resources;
        if (!r) return;
        const names = this.draws.map(({ name }) => name).join(' ');
        if (names !== this.filled && this.draws.some(({ name }) => !this.placed.has(name))) {
            this.filled = names;
            this.placed.clear();
            let vertexBytes = 0, indexBytes = 0;
            for (const { name, mesh } of this.draws) {
                if (vertexBytes + mesh.vertices.byteLength > VERTEX_BYTES || indexBytes + mesh.indices.byteLength > INDEX_BYTES) continue;
                r.device.queue.writeBuffer(r.vertices, vertexBytes, mesh.vertices);
                r.device.queue.writeBuffer(r.indices, indexBytes, mesh.indices);
                this.placed.set(name, { first: indexBytes / 4, base: vertexBytes / (WATER_VERTEX * 4), count: mesh.indices.length });
                vertexBytes += mesh.vertices.byteLength;
                indexBytes += mesh.indices.byteLength;
            }
        }
        const instances = this.draws.flatMap(({ name, nodes }) => (this.placed.has(name) ? nodes : []));
        if (instances.length) r.device.queue.writeBuffer(r.instances, 0, new Uint32Array(instances));
    }

    draw(pass: GPURenderPassEncoder, surfaceGroup: GPUBindGroup): void {
        const r = this.resources;
        if (!r || !this.draws.length) return;
        pass.setPipeline(r.pipeline);
        pass.setBindGroup(0, r.group);
        pass.setBindGroup(1, surfaceGroup);
        pass.setVertexBuffer(0, r.vertices);
        pass.setIndexBuffer(r.indices, 'uint32');
        let instance = 0;
        for (const { name, nodes } of this.draws) {
            const at = this.placed.get(name);
            if (!at) continue;
            pass.drawIndexed(at.count, nodes.length, at.first, at.base, instance);
            instance += nodes.length;
        }
    }
}
