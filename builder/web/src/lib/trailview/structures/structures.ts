import type { Gpu } from '../gpu/device';
import { layouts } from '../gpu/layouts';
import { wgsl } from '../gpu/wgsl';
import { CASTER_DEPTH, CASTER_PRIMITIVE, NEAR_CASTERS_M } from '../light/shadow';
import { DEPTH, sceneFormat } from '../resolve/targets';
import { metresPerUnit } from '../terrain/bins';
import { key, size, type Node } from '../terrain/lod';
import type { Stamps } from '../terrain/stamps';
import { ANCHOR_WORDS, ANCHORED_VERTEX } from './anchored';
import { STRUCTURE_VERTEX } from './mesh';
import { FLOOR, OWNER, type NodeMeshes } from './node';

/**
 * The structure meshes and the anchored meshes on the GPU, fixed pools; the anchors of the anchored
 * meshes that a frame draws, and their heights.
 */
const VERTEX_BYTES = 8 * 2 ** 20, INDEX_BYTES = 4 * 2 ** 20, ANCHORED_BYTES = 6 * 2 ** 20, ANCHORED_INDEX_BYTES = 3 * 2 ** 20, ANCHORS = 2 ** 16;
/**
 * A floor reaches this far past the ends of its segments: off a deck or out of a tunnel the ground holds
 * the road's height, and the probe of the ground under the eye, a few frames late, catches up meanwhile.
 */
const REACH_M = 3;
/** An OWNER node is near while a node of this level or finer draws in it: within about 2 km. */
const NEAR_LEVEL = 17;
/** Anchor ends per workgroup of `anchor_heights`. */
const ANCHOR_GROUP = 64;

interface Mesh { vertices: ArrayBuffer; indices: Uint32Array<ArrayBuffer> }

/**
 * Meshes in a vertex and an index buffer of fixed size. A new mesh goes after the others; when one does
 * not fit and the pool holds meshes that the frame does not draw, the pool starts again with the meshes
 * of the frame.
 */
class Pool {
    private readonly placed = new Map<Mesh, { first: number; base: number }>();
    private vertexEnd = 0;
    private indexEnd = 0;
    /** The meshes of the last frame that did not fit. */
    dropped = 0;

    constructor(private readonly device: GPUDevice, readonly vertices: GPUBuffer, readonly indices: GPUBuffer, private readonly stride: number) {}

    write(draws: Mesh[]): void {
        const placeAll = () => {
            for (const mesh of draws) if (!this.placed.has(mesh)) this.place(mesh);
        };
        const missing = () => draws.filter(mesh => !this.placed.has(mesh)).length;
        placeAll();
        const used = new Set(draws);
        if (missing() && [...this.placed.keys()].some(mesh => !used.has(mesh))) {
            this.placed.clear();
            this.vertexEnd = this.indexEnd = 0;
            placeAll();
        }
        this.dropped = missing();
    }

    at(mesh: Mesh): { first: number; base: number } | undefined {
        return this.placed.get(mesh);
    }

    private place(mesh: Mesh): void {
        if (this.vertexEnd + mesh.vertices.byteLength > this.vertices.size || this.indexEnd + mesh.indices.byteLength > this.indices.size) return;
        this.device.queue.writeBuffer(this.vertices, this.vertexEnd, mesh.vertices);
        this.device.queue.writeBuffer(this.indices, this.indexEnd, mesh.indices);
        this.placed.set(mesh, { first: this.indexEnd / 4, base: this.vertexEnd / (this.stride * 4) });
        this.vertexEnd += mesh.vertices.byteLength;
        this.indexEnd += mesh.indices.byteLength;
    }
}

interface Resources {
    device: GPUDevice;
    pipeline: GPURenderPipeline;
    anchoredPipeline: GPURenderPipeline;
    /** The pipelines of the structure and the anchored meshes as casters of the shadow map, with the group of the sun's frame. */
    casters: [GPURenderPipeline, GPURenderPipeline];
    sunGroup: GPUBindGroup;
    heightsPipeline: GPUComputePipeline;
    group: GPUBindGroup;
    computeGroup: GPUBindGroup;
    emptyGroup: GPUBindGroup;
    structures: Pool;
    anchored: Pool;
    anchors: GPUBuffer;
    count: GPUBuffer;
}

/**
 * The structures, with structures.wgsl: the meshes of each OWNER node that a drawn node of that level
 * or finer lies in, once; the near meshes too where a drawn node of NEAR_LEVEL lies in it. They draw
 * before the terrain, so the terrain behind them shades nothing.
 */
export class Structures {
    private resources?: Resources;
    private draws: Mesh[] = [];
    /** The anchored meshes of the frame, with the first anchor of their node. */
    private hung: { mesh: Mesh; anchor: number }[] = [];
    private anchorWords = new Uint32Array(0);
    /** The models on the surface of the kinds of the drawn nodes, a list per node. */
    points: Uint32Array[] = [];
    /** The meshes of the drawn owners, and whether each is near: the last frame's, to compare. */
    private drawn: { meshes: NodeMeshes; near: boolean }[] = [];
    /** The anchored meshes of the frame whose anchors did not fit. */
    private unanchored = 0;
    /** Whether the anchors of the frame are not on the GPU yet. */
    private fresh = false;

    constructor(private readonly stamps: Stamps) {}

    /** The meshes of the last frame that did not fit in the pools, or whose anchors did not fit. */
    get dropped(): number {
        return (this.resources?.structures.dropped ?? 0) + (this.resources?.anchored.dropped ?? 0) + this.unanchored;
    }

    /** `frames` are the frame uniforms of the eye's view and of the sun's (light/shadow.ts). */
    async attach(gpu: Gpu, [frame, sunFrame]: GPUBuffer[]): Promise<void> {
        this.resources = undefined;
        const { device } = gpu, { layout, empty, computeLayout, pipelines: [pipeline, anchoredPipeline, heightsPipeline, structureCaster, anchoredCaster] } = await gpu.once(structurePipelines);
        const anchors = gpu.buffer('kinds', ANCHORS * ANCHOR_WORDS * 4, GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST);
        const heights = gpu.buffer('kinds', ANCHORS * 8, GPUBufferUsage.STORAGE);
        const count = gpu.buffer('kinds', 16, GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST);
        const pool = (label: string, vertexBytes: number, indexBytes: number, stride: number) =>
            new Pool(device, gpu.buffer(label, vertexBytes, GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST), gpu.buffer(label, indexBytes, GPUBufferUsage.INDEX | GPUBufferUsage.COPY_DST), stride);
        this.drawn = [];
        this.fresh = true;
        const frameGroup = (frame: GPUBuffer) => device.createBindGroup({ layout, entries: [[0, frame], [1, anchors], [4, heights]].map(([binding, buffer]) => ({ binding: binding as number, resource: { buffer: buffer as GPUBuffer } })) });
        this.resources = {
            device, pipeline, anchoredPipeline, casters: [structureCaster, anchoredCaster], heightsPipeline, anchors, count,
            group: frameGroup(frame), sunGroup: frameGroup(sunFrame),
            emptyGroup: device.createBindGroup({ layout: empty, entries: [] }),
            computeGroup: device.createBindGroup({ layout: computeLayout, entries: [frame, anchors, heights, count].map((buffer, binding) => ({ binding, resource: { buffer } })) }),
            structures: pool('structures', VERTEX_BYTES, INDEX_BYTES, STRUCTURE_VERTEX),
            anchored: pool('kinds', ANCHORED_BYTES, ANCHORED_INDEX_BYTES, ANCHORED_VERTEX),
        };
    }

    /**
     * The meshes of a frame from its drawn nodes, which ask for them, and from the nodes whose structures
     * cast into the near shadow map about the eye (world units, and metres per unit). True when they
     * differ from the last frame's.
     */
    update(drawn: Node[], eye: { x: number; y: number; unit: number }): boolean {
        const owners = new Map<string, { node: Node; near: boolean }>();
        for (const { level, x, y } of drawn) {
            if (level < OWNER) continue;
            const node = { level: OWNER, x: x >> (level - OWNER), y: y >> (level - OWNER) }, name = key(node);
            owners.set(name, { node, near: owners.get(name)?.near || level >= NEAR_LEVEL });
        }
        const side = size(OWNER), reach = NEAR_CASTERS_M / eye.unit;
        for (let y = Math.floor((eye.y - reach) / side); y <= Math.floor((eye.y + reach) / side); y++) {
            for (let x = Math.floor((eye.x - reach) / side); x <= Math.floor((eye.x + reach) / side); x++) owners.set(key({ level: OWNER, x, y }), { node: { level: OWNER, x, y }, near: true });
        }
        const now: { meshes: NodeMeshes; near: boolean }[] = [];
        for (const { node, near } of owners.values()) {
            const meshes = this.stamps.get(node)?.meshes;
            if (meshes) now.push({ meshes, near });
        }
        if (now.length === this.drawn.length && now.every((o, i) => o.meshes === this.drawn[i].meshes && o.near === this.drawn[i].near)) return false;
        this.drawn = now;
        this.fresh = true;
        const words: Uint32Array[] = [];
        let anchor = 0;
        this.draws = [];
        this.hung = [];
        this.points = [];
        this.unanchored = 0;
        for (const { meshes, near } of now) {
            const parts = near ? [meshes.far, meshes.near] : [meshes.far];
            // The models on the surface reach no farther than the near meshes.
            if (near && meshes.points) this.points.push(meshes.points);
            for (const { structures } of parts) if (structures) this.draws.push(structures);
            const count = (meshes.anchors?.byteLength ?? 0) / (ANCHOR_WORDS * 4), hung = parts.map(({ anchored }) => anchored).filter(mesh => mesh !== null);
            if (!meshes.anchors) continue;
            if (anchor + count > ANCHORS) {
                this.unanchored += hung.length;
                continue;
            }
            for (const mesh of hung) this.hung.push({ mesh, anchor });
            words.push(new Uint32Array(meshes.anchors));
            anchor += count;
        }
        this.anchorWords = new Uint32Array(anchor * ANCHOR_WORDS);
        let at = 0;
        for (const list of words) {
            this.anchorWords.set(list, at);
            at += list.length;
        }
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

    /** Uploads the meshes that the pools lack, and the anchors of the frame. */
    write(): void {
        const r = this.resources;
        if (!r) return;
        r.structures.write(this.draws);
        r.anchored.write(this.hung.map(({ mesh }) => mesh));
        if (!this.fresh) return;
        this.fresh = false;
        if (this.anchorWords.length) r.device.queue.writeBuffer(r.anchors, 0, this.anchorWords);
        r.device.queue.writeBuffer(r.count, 0, new Uint32Array([this.anchorWords.length / ANCHOR_WORDS, 0, 0, 0]));
    }

    /** Encodes the heights of the frame's anchors on the drawn surface into a compute pass before the draw. */
    encode(pass: GPUComputePassEncoder, surfaceGroup: GPUBindGroup, features: GPUBindGroup): void {
        const r = this.resources, count = this.anchorWords.length / ANCHOR_WORDS;
        if (!r || !count) return;
        pass.setPipeline(r.heightsPipeline);
        pass.setBindGroup(0, r.computeGroup);
        pass.setBindGroup(1, surfaceGroup);
        pass.setBindGroup(2, r.emptyGroup);
        pass.setBindGroup(3, features);
        pass.dispatchWorkgroups(Math.ceil(2 * count / ANCHOR_GROUP));
    }

    draw(pass: GPURenderPassEncoder, surfaceGroup: GPUBindGroup, ground: GPUBindGroup, features: GPUBindGroup): void {
        const r = this.resources;
        if (r) this.drawWith(pass, [r.group, surfaceGroup, ground, features], r.pipeline, r.anchoredPipeline);
    }

    /** Draws the meshes into the shadow map (light/shadow.ts) as casters. */
    cast(pass: GPURenderPassEncoder, surfaceGroup: GPUBindGroup, features: GPUBindGroup): void {
        const r = this.resources;
        if (r) this.drawWith(pass, [r.sunGroup, surfaceGroup, r.emptyGroup, features], ...r.casters);
    }

    private drawWith(pass: GPURenderPassEncoder, groups: GPUBindGroup[], structures: GPURenderPipeline, anchored: GPURenderPipeline): void {
        const r = this.resources!;
        if (!this.draws.length && !this.hung.length) return;
        groups.forEach((group, k) => pass.setBindGroup(k, group));
        pass.setPipeline(structures);
        pass.setVertexBuffer(0, r.structures.vertices);
        pass.setIndexBuffer(r.structures.indices, 'uint32');
        for (const mesh of this.draws) {
            const at = r.structures.at(mesh);
            if (at) pass.drawIndexed(mesh.indices.length, 1, at.first, at.base);
        }
        pass.setPipeline(anchored);
        pass.setVertexBuffer(0, r.anchored.vertices);
        pass.setIndexBuffer(r.anchored.indices, 'uint32');
        // The first instance names the first anchor of the mesh's node.
        for (const { mesh, anchor } of this.hung) {
            const at = r.anchored.at(mesh);
            if (at) pass.drawIndexed(mesh.indices.length, 1, at.first, at.base, anchor);
        }
    }
}

/** The draws of the structures and the anchored meshes, the heights of the anchors, the casters, and their layouts. */
async function structurePipelines(gpu: Gpu) {
    const { device } = gpu, VF = GPUShaderStage.VERTEX | GPUShaderStage.FRAGMENT, C = GPUShaderStage.COMPUTE, { surface, ground, features } = gpu.once(layouts);
    // The heights are written in the compute's group and read in the draw's.
    const layout = device.createBindGroupLayout({ entries: [
        { binding: 0, visibility: VF, buffer: { type: 'uniform' } }, { binding: 1, visibility: GPUShaderStage.VERTEX, buffer: { type: 'read-only-storage' } },
        { binding: 4, visibility: GPUShaderStage.VERTEX, buffer: { type: 'read-only-storage' } },
    ] });
    const empty = device.createBindGroupLayout({ entries: [] }), computeLayout = device.createBindGroupLayout({ entries: [
        { binding: 0, visibility: C, buffer: { type: 'uniform' } }, { binding: 1, visibility: C, buffer: { type: 'read-only-storage' } },
        { binding: 2, visibility: C, buffer: { type: 'storage' } }, { binding: 3, visibility: C, buffer: { type: 'uniform' } },
    ] });
    const module = device.createShaderModule({ label: 'structures', code: wgsl('structures/structures.wgsl') });
    const pipelineLayout = device.createPipelineLayout({ bindGroupLayouts: [layout, surface, ground, features] });
    const render = (entryPoint: string, buffers: GPUVertexBufferLayout[], cullMode: GPUCullMode) => device.createRenderPipelineAsync({
        label: entryPoint, layout: pipelineLayout, vertex: { module, entryPoint, buffers },
        fragment: { module, entryPoint: 'structure_fragment', targets: [{ format: sceneFormat(gpu) }] },
        primitive: { topology: 'triangle-list', cullMode },
        depthStencil: { format: DEPTH, depthWriteEnabled: true, depthCompare: 'greater' },
    });
    const structureVertices: GPUVertexBufferLayout[] = [{
        arrayStride: STRUCTURE_VERTEX * 4, attributes: [
            { shaderLocation: 0, offset: 0, format: 'sint32x2' }, { shaderLocation: 1, offset: 8, format: 'float32' },
            { shaderLocation: 2, offset: 12, format: 'unorm8x4' }, { shaderLocation: 3, offset: 16, format: 'snorm16x2' },
            { shaderLocation: 4, offset: 20, format: 'float32' },
        ],
    }];
    const anchoredVertices: GPUVertexBufferLayout[] = [{
        arrayStride: ANCHORED_VERTEX * 4, attributes: [
            { shaderLocation: 0, offset: 0, format: 'sint16x4' }, { shaderLocation: 1, offset: 8, format: 'uint32' }, { shaderLocation: 2, offset: 12, format: 'unorm8x4' },
        ],
    }];
    // The vertices read no group 2, which holds the shadow map that the casters draw.
    const casterLayout = device.createPipelineLayout({ bindGroupLayouts: [layout, surface, empty, features] });
    const caster = (entryPoint: string, buffers: GPUVertexBufferLayout[]) => device.createRenderPipelineAsync({
        label: `${entryPoint} caster`, layout: casterLayout, vertex: { module, entryPoint, buffers }, primitive: CASTER_PRIMITIVE, depthStencil: CASTER_DEPTH,
    });
    const pipelines = await Promise.all([
        render('structure_vertex', structureVertices, 'back'),
        // Models need no winding: both faces draw.
        render('anchored_vertex', anchoredVertices, 'none'),
        device.createComputePipelineAsync({
            label: 'anchor heights', layout: device.createPipelineLayout({ bindGroupLayouts: [computeLayout, surface, empty, features] }),
            compute: { module, entryPoint: 'anchor_heights' },
        }),
        caster('structure_vertex', structureVertices), caster('anchored_caster', anchoredVertices),
    ]);
    return { layout, empty, computeLayout, pipelines };
}
