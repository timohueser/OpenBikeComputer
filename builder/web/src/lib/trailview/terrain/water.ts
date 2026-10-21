import type { Gpu } from '../gpu/device';
import { wgsl } from '../gpu/wgsl';
import { COARSEST, key, type Node } from './lod';
import type { Stamps } from './stamps';
import { WATER_VERTEX, type WaterMesh } from './water-mesh';

/**
 * The water meshes on the GPU, a fixed pool. A new mesh goes after the others; when one does not fit
 * and the pool holds meshes that the frame does not draw, the pool starts again with the meshes of the
 * frame, finest first.
 */
const VERTEX_BYTES = 4 * 2 ** 20, INDEX_BYTES = 2 * 2 ** 20;

/** A mesh, the level of the node that it belongs to, and the drawn nodes that draw it. */
interface Draw { mesh: WaterMesh; level: number; nodes: number[] }

interface Resources {
    device: GPUDevice;
    lakes: GPURenderPipeline;
    ribbons: GPURenderPipeline;
    group: GPUBindGroup;
    vertices: GPUBuffer;
    indices: GPUBuffer;
    instances: GPUBuffer;
}

/**
 * The water of the drawn nodes, with water.wgsl: each draws the mesh of the node whose page it draws,
 * or of that node's nearest ancestor whose mesh is known. The lakes draw opaque and
 * write their depth, so the GPU skips the terrain under them and the ribbons under a lake stay hidden;
 * then the ribbons blend.
 */
export class Water {
    private resources?: Resources;
    private draws: Draw[] = [];
    /** Per mesh in the pool: its first index and its first vertex. */
    private placed = new Map<WaterMesh, { first: number; base: number }>();
    private vertexEnd = 0;
    private indexEnd = 0;
    /** The meshes of the last frame that did not fit in the pool. */
    dropped = 0;

    constructor(private readonly stamps: Stamps) {}

    async attach(gpu: Gpu, frame: GPUBuffer, nodes: GPUBuffer, surfaceLayout: GPUBindGroupLayout, groundLayout: GPUBindGroupLayout, depthFormat: GPUTextureFormat): Promise<void> {
        const { device } = gpu, V = GPUShaderStage.VERTEX;
        const layout = device.createBindGroupLayout({ entries: [
            { binding: 0, visibility: V | GPUShaderStage.FRAGMENT, buffer: { type: 'uniform' } },
            { binding: 2, visibility: V, buffer: { type: 'read-only-storage' } },
            { binding: 3, visibility: V, buffer: { type: 'read-only-storage' } },
        ] });
        // A node index and a level per instance, at most one instance per node.
        const instances = gpu.buffer('water', nodes.size / 2, GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST);
        const module = device.createShaderModule({ label: 'water', code: wgsl('terrain/water.wgsl') });
        const blend: GPUBlendComponent = { srcFactor: 'src-alpha', dstFactor: 'one-minus-src-alpha' };
        const pipeline = (ribbons: boolean) => device.createRenderPipelineAsync({
            label: ribbons ? 'ribbons' : 'lakes', layout: device.createPipelineLayout({ bindGroupLayouts: [layout, surfaceLayout, groundLayout] }),
            vertex: {
                module, entryPoint: 'water_vertex', buffers: [{
                    arrayStride: WATER_VERTEX * 4, attributes: [
                        { shaderLocation: 0, offset: 0, format: 'sint32x2' }, { shaderLocation: 1, offset: 8, format: 'float32' },
                        { shaderLocation: 2, offset: 12, format: 'float32x2' }, { shaderLocation: 3, offset: 20, format: 'float32' },
                    ],
                }],
            },
            fragment: { module, entryPoint: 'water_fragment', targets: [{ format: gpu.format, ...(ribbons ? { blend: { color: blend, alpha: blend } } : {}) }] },
            primitive: { topology: 'triangle-list' },
            depthStencil: { format: depthFormat, depthWriteEnabled: !ribbons, depthCompare: 'greater' },
        });
        const [lakes, ribbons] = await Promise.all([pipeline(false), pipeline(true)]);
        this.placed.clear();
        this.vertexEnd = this.indexEnd = 0;
        this.resources = {
            device, lakes, ribbons, instances,
            group: device.createBindGroup({ layout, entries: [{ binding: 0, resource: { buffer: frame } }, { binding: 2, resource: { buffer: nodes } }, { binding: 3, resource: { buffer: instances } }] }),
            vertices: gpu.buffer('water', VERTEX_BYTES, GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST),
            indices: gpu.buffer('water', INDEX_BYTES, GPUBufferUsage.INDEX | GPUBufferUsage.COPY_DST),
        };
    }

    /** The draws of a frame: per mesh, the drawn nodes that draw it. `drawing` has the node of the page that each drawn node draws. */
    update(drawing: Node[]): void {
        const draws = new Map<string, Draw>();
        drawing.forEach((node, i) => {
            for (let { level, x, y } = node; level >= COARSEST; level--, x >>= 1, y >>= 1) {
                const known = this.stamps.known({ level, x, y });
                if (!known) continue;
                if (!known.water) return;
                const name = key({ level, x, y }), draw = draws.get(name) ?? { mesh: known.water, level, nodes: [] };
                draws.set(name, draw);
                draw.nodes.push(i);
                return;
            }
        });
        this.draws = [...draws.values()].sort((a, b) => b.level - a.level);
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
        // Without a mesh to free, the pool keeps its meshes: starting again would upload them every frame.
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

    private place(r: Resources, mesh: WaterMesh): boolean {
        if (this.vertexEnd + mesh.vertices.byteLength > VERTEX_BYTES || this.indexEnd + mesh.indices.byteLength > INDEX_BYTES) return false;
        r.device.queue.writeBuffer(r.vertices, this.vertexEnd, mesh.vertices);
        r.device.queue.writeBuffer(r.indices, this.indexEnd, mesh.indices);
        this.placed.set(mesh, { first: this.indexEnd / 4, base: this.vertexEnd / (WATER_VERTEX * 4) });
        this.vertexEnd += mesh.vertices.byteLength;
        this.indexEnd += mesh.indices.byteLength;
        return true;
    }

    draw(pass: GPURenderPassEncoder, surfaceGroup: GPUBindGroup, ground: GPUBindGroup): void {
        const r = this.resources;
        if (!r || !this.draws.length) return;
        pass.setBindGroup(0, r.group);
        pass.setBindGroup(1, surfaceGroup);
        pass.setBindGroup(2, ground);
        pass.setVertexBuffer(0, r.vertices);
        pass.setIndexBuffer(r.indices, 'uint32');
        for (const ribbons of [false, true]) {
            pass.setPipeline(ribbons ? r.ribbons : r.lakes);
            let instance = 0;
            for (const { mesh, nodes } of this.draws) {
                const at = this.placed.get(mesh);
                if (!at) continue;
                const [first, count] = ribbons ? [at.first, mesh.ribbons] : [at.first + mesh.ribbons, mesh.indices.length - mesh.ribbons];
                if (count) pass.drawIndexed(count, nodes.length, first, at.base, instance);
                instance += nodes.length;
            }
        }
    }
}
