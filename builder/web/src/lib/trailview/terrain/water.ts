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
/** The depth bias of water on the drawn surface, per unit of its depth slope. */
const DRAPE_BIAS = 2;

/**
 * The kinds of water in the order that they draw. A lake is opaque and writes its depth, so the GPU skips
 * the terrain under it and the water under a lake stays hidden; a river area is opaque too. Water on the
 * drawn surface takes a depth bias with its slope, so that the terrain's triangles between its vertices
 * stay under it; the ribbons blend last.
 */
const KINDS = ['lakes', 'areas', 'ribbons'] as const;
type Kind = (typeof KINDS)[number];

/** A mesh, the level of the node that it belongs to, and the drawn nodes that draw it. */
interface Draw { mesh: WaterMesh; level: number; nodes: number[] }

interface Resources {
    device: GPUDevice;
    /** Per kind of water, in the order that they draw. */
    pipelines: Record<Kind, GPURenderPipeline>;
    group: GPUBindGroup;
    vertices: GPUBuffer;
    indices: GPUBuffer;
    instances: GPUBuffer;
}

/**
 * The water of the drawn nodes, with water.wgsl: each draws the mesh of the node whose page it draws,
 * or of that node's nearest ancestor whose mesh is known.
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
    private drawn = '';

    constructor(private readonly stamps: Stamps) {}

    async attach(gpu: Gpu, frame: GPUBuffer, nodes: GPUBuffer, surfaceLayout: GPUBindGroupLayout, groundLayout: GPUBindGroupLayout, featureLayout: GPUBindGroupLayout, format: GPUTextureFormat, depthFormat: GPUTextureFormat): Promise<void> {
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
        const pipeline = (kind: Kind) => device.createRenderPipelineAsync({
            label: kind, layout: device.createPipelineLayout({ bindGroupLayouts: [layout, surfaceLayout, groundLayout, featureLayout] }),
            vertex: {
                module, entryPoint: 'water_vertex', buffers: [{
                    arrayStride: WATER_VERTEX * 4, attributes: [
                        { shaderLocation: 0, offset: 0, format: 'sint32x2' }, { shaderLocation: 1, offset: 8, format: 'float32' },
                        { shaderLocation: 2, offset: 12, format: 'float32x2' }, { shaderLocation: 3, offset: 20, format: 'float32' },
                    ],
                }],
            },
            fragment: { module, entryPoint: 'water_fragment', targets: [{ format, ...(kind === 'ribbons' ? { blend: { color: blend, alpha: blend } } : {}) }] },
            primitive: { topology: 'triangle-list' },
            depthStencil: { format: depthFormat, depthWriteEnabled: kind !== 'ribbons', depthCompare: kind === 'ribbons' ? 'greater-equal' : 'greater', depthBiasSlopeScale: kind === 'lakes' ? 0 : DRAPE_BIAS },
        });
        const pipelines = Object.fromEntries(await Promise.all(KINDS.map(async kind => [kind, await pipeline(kind)]))) as Record<Kind, GPURenderPipeline>;
        this.placed.clear();
        this.vertexEnd = this.indexEnd = 0;
        this.resources = {
            device, pipelines, instances,
            group: device.createBindGroup({ layout, entries: [{ binding: 0, resource: { buffer: frame } }, { binding: 2, resource: { buffer: nodes } }, { binding: 3, resource: { buffer: instances } }] }),
            vertices: gpu.buffer('water', VERTEX_BYTES, GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST),
            indices: gpu.buffer('water', INDEX_BYTES, GPUBufferUsage.INDEX | GPUBufferUsage.COPY_DST),
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
                if (!known.water) return;
                const name = key({ level, x, y }), draw = draws.get(name) ?? { mesh: known.water, level, nodes: [] };
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

    draw(pass: GPURenderPassEncoder, surfaceGroup: GPUBindGroup, ground: GPUBindGroup, features: GPUBindGroup): void {
        const r = this.resources;
        if (!r || !this.draws.length) return;
        pass.setBindGroup(0, r.group);
        pass.setBindGroup(1, surfaceGroup);
        pass.setBindGroup(2, ground);
        pass.setBindGroup(3, features);
        pass.setVertexBuffer(0, r.vertices);
        pass.setIndexBuffer(r.indices, 'uint32');
        for (const kind of KINDS) {
            pass.setPipeline(r.pipelines[kind]);
            let instance = 0;
            for (const { mesh, nodes } of this.draws) {
                const at = this.placed.get(mesh);
                if (!at) continue;
                const [first, count] = ({
                    ribbons: [0, mesh.ribbons], lakes: [mesh.ribbons, mesh.lakes], areas: [mesh.ribbons + mesh.lakes, mesh.indices.length - mesh.ribbons - mesh.lakes],
                } as const)[kind];
                if (count) pass.drawIndexed(count, nodes.length, at.first + first, at.base, instance);
                instance += nodes.length;
            }
        }
    }
}
