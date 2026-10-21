import type { Bounds } from '../../planner/config';
import { worldPixel } from '../../planner/layers/mercator';
import { VFOV } from '../frame/camera';
import { CURVATURE, EARTH_RADIUS, tileLonLat, type Frame } from '../frame/frame';
import { anchorAt, cubic, lodSpace, renderMap, UNITS, type Anchor } from '../frame/space';
import type { Gpu } from '../gpu/device';
import { wgsl } from '../gpu/wgsl';
import type { Labs } from '../labs';
import { PAGE } from '../stream/heights';
import type { TerrainSource } from '../stream/terrain';
import { cdlod, distances, FINEST, GRID, MAP, RANGE, RINGS, rings, select, size, type Node, type Rule } from './lod';
import { ARRIVALS, Pages, type Kind, type Page, type Work } from './pages';

/** GPU memory for pages, heights and normals: the stored and the synthesized budgets for a laptop, in MiB. */
const BUDGET: Record<Kind, number> = { stored: 240 * 2 ** 20, synth: 50 * 2 ** 20 };
/** Nodes drawn per frame at most. */
const MAX_NODES = 4096;
/** The grid cell that a level has at the far end of its range, in pixels. */
const CELL_PIXELS = 4;
/** Frames without a shortage before the quality rises again, when each pool would still have room: the share that it fills at most. */
const CALM = 120, ROOM = 0.6;
/** Heights that a node of unknown range may have, and the margin about a known range: micro-relief and bicubic overshoot. */
const ANY_HEIGHT: [number, number] = [-500, 9000];
const MARGIN = 50;
/** Pages per texture array layer: 8 × 8, so a device with the least layers that WebGPU allows holds every pool. */
const LAYER_PAGES = 64;
/** Points of a check at most. */
const CHECKS = 64;
/** The probe reads the eye and four points this many metres east, west, north and south of it, for the slope. */
const PROBE_STEP = 1;
const READBACKS = 3;
const RAD = Math.PI / 180;
/** Bytes of `Surface` and `Job` in the WGSL, and of a node instance. */
const SURFACE_BYTES = 416, JOB_BYTES = 32, NODE_BYTES = 16;
const GRID_INDICES = gridIndices();

/** What the terrain draws from: the eye, its height above the ground, and the camera. */
export interface View {
    lonLat: [number, number];
    /** Metres above the ground; over an unknown ground, the pose's height. */
    above: number;
    /** Metres above sea level, when known. */
    z?: number;
    viewProj: Float32Array;
    /** Pixels of the render target's height. */
    pixels: number;
}

export interface TerrainStats {
    nodes: number;
    triangles: number;
    /** Milliseconds of the CPU selection in the last frame. */
    selection: number;
    resident: Record<Kind, number>;
    capacity: Record<Kind, number>;
    /** 0 is the best; each step draws coarser to stay in the pools. */
    quality: number;
}

interface Resources {
    gpu: Gpu;
    heights: GPUTexture;
    normals: GPUTexture;
    surface: GPUBuffer;
    map: GPUBuffer;
    scales: GPUBuffer;
    nodes: GPUBuffer;
    synthJobs: GPUBuffer;
    normalJobs: GPUBuffer;
    synthOut: GPUBuffer;
    normalOut: GPUBuffer;
    extent: GPUBuffer;
    codes: GPUBuffer;
    indices: GPUBuffer;
    points: GPUBuffer;
    probed: GPUBuffer;
    readbacks: GPUBuffer[];
    surfaceGroup: GPUBindGroup;
    meshGroup: GPUBindGroup;
    synthGroup: GPUBindGroup;
    normalGroup: GPUBindGroup;
    quantizeGroup: GPUBindGroup;
    probeGroup: GPUBindGroup;
    mesh: GPURenderPipeline;
    synthesize: GPUComputePipeline;
    normalsOf: GPUComputePipeline;
    quantize: GPUComputePipeline;
    probe: GPUComputePipeline;
    check: GPURenderPipeline;
    probeLayout: GPUBindGroupLayout;
    checkLayout: GPUBindGroupLayout;
}

/**
 * The terrain: CDLOD by distance or by rings (the `terrain` lab) over pages of the stored terrain and of
 * levels synthesized below it, in fixed pools. One instanced grid mesh draws every node.
 */
export class Terrain {
    readonly pages: Pages;
    /**
     * The probed ground under the eye at `lonLat`: NaN where no level knows it, a slope of 0 where no
     * level knows a point about it, and the frame that it comes from.
     */
    ground?: { height: number; slope: [number, number]; lonLat: [number, number]; frame: number };
    private resources?: Resources;
    private readonly bounds: [number, number, number, number];
    private nodes: Node[] = [];
    private work?: Work;
    private anchor?: Anchor;
    private quality = 0;
    private calm = 0;
    private selection = 0;
    private frameNumber = 0;
    private probing = 0;
    /** The state that the last probe read, and a count of the frames that changed the pages. */
    private probed = '';
    private changes = 0;
    private above = 0;
    private lonLat: [number, number] = [0, 0];
    private floor = 0;

    constructor(private readonly frame: Frame, source: TerrainSource, bounds: Bounds, private readonly labs: Labs, private readonly changed: () => void) {
        const [west, north] = worldPixel([bounds[0], bounds[3]], UNITS), [east, south] = worldPixel([bounds[2], bounds[1]], UNITS);
        this.bounds = [west, north, east, south];
        const bytes = PAGE * PAGE * ((labs.pages === 'r16' ? 2 : 4) + 2);
        // Whole layers inside the two budgets; the synthesized pool takes its share, the stored pool the rest.
        const pages = Math.floor((BUDGET.stored + BUDGET.synth) / bytes / LAYER_PAGES) * LAYER_PAGES, synth = Math.floor(BUDGET.synth / bytes);
        this.pages = new Pages(source, { stored: pages - synth, synth }, this.bounds, changed);
    }

    get idle(): boolean {
        return this.pages.idle && this.calm > 0 && this.probing === 0;
    }

    /** True while the terrain needs frames that no tile load or probe will ask for. */
    get working(): boolean {
        return (!this.pages.idle && this.pages.loads === 0) || this.calm === 0;
    }

    /** A new view: a probe from before it never sets the ground. */
    forget(): void {
        this.ground = undefined;
        this.floor = this.frameNumber;
    }

    /** Nodes drawn in the last frame. */
    get drawn(): number {
        return this.nodes.length;
    }

    get stats(): TerrainStats {
        const resident = { stored: 0, synth: 0 };
        for (const page of this.pages.pages.values()) if (page.ready) resident[page.kind]++;
        return { nodes: this.nodes.length, triangles: 2 * GRID * GRID * this.nodes.length, selection: this.selection, resident, capacity: { ...this.pages.capacity }, quality: this.quality };
    }

    /** Compiles the pipelines for a device. A new device gets every page again, from the kept tile bodies. */
    async attach(gpu: Gpu, frameUniform: GPUBuffer, depthFormat: GPUTextureFormat): Promise<void> {
        this.resources = undefined;
        this.pages.lost();
        const { device } = gpu, labs = this.labs, pages = this.pages.capacity.stored + this.pages.capacity.synth;
        const layers = Math.ceil(pages / LAYER_PAGES), side = PAGE * Math.sqrt(LAYER_PAGES);
        const r16 = labs.pages === 'r16', constants = { RINGS: labs.terrain === 'rings' ? 1 : 0, R16: r16 ? 1 : 0 };
        const texels = PAGE * PAGE;
        const usage = GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST;
        const heights = gpu.texture('pages', { size: [side, side, layers], format: r16 ? 'r16uint' : 'r32uint', usage }, layers * side * side * (r16 ? 2 : 4));
        const normals = gpu.texture('pages', { size: [side, side, layers], format: 'rg8snorm', usage }, layers * side * side * 2);
        const storage = GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST;
        const buffer = (pool: string, bytes: number, usage: number) => gpu.buffer(pool, bytes, usage);
        const r: Omit<Resources, 'surfaceGroup' | 'meshGroup' | 'synthGroup' | 'normalGroup' | 'quantizeGroup' | 'probeGroup' | 'mesh' | 'synthesize' | 'normalsOf' | 'quantize' | 'probe' | 'check' | 'probeLayout' | 'checkLayout'> = {
            gpu, heights, normals,
            surface: buffer('terrain', SURFACE_BYTES, GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST),
            map: buffer('terrain', (FINEST + 1) * MAP * MAP * 4, storage),
            scales: buffer('terrain', pages * 8, storage),
            nodes: buffer('terrain', MAX_NODES * NODE_BYTES, storage),
            synthJobs: buffer('compute', ARRIVALS * JOB_BYTES, storage),
            normalJobs: buffer('compute', 2 * ARRIVALS * JOB_BYTES, storage),
            synthOut: buffer('compute', ARRIVALS * texels * 4, GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC),
            normalOut: buffer('compute', 2 * ARRIVALS * texels * 2, GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC),
            extent: buffer('compute', ARRIVALS * 8, storage),
            codes: buffer('compute', r16 ? ARRIVALS * texels * 2 : 4, GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC),
            indices: buffer('terrain', GRID_INDICES.byteLength, GPUBufferUsage.INDEX | GPUBufferUsage.COPY_DST),
            // The eye and four points about it, for the slope.
            points: buffer('probe', 40, storage),
            probed: buffer('probe', 20, GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC),
            readbacks: Array.from({ length: READBACKS }, () => buffer('probe', 20, GPUBufferUsage.MAP_READ | GPUBufferUsage.COPY_DST)),
        };
        device.queue.writeBuffer(r.indices, 0, GRID_INDICES);
        device.queue.writeBuffer(r.scales, 0, new Float32Array(2 * pages).fill(1));

        const all = GPUShaderStage.VERTEX | GPUShaderStage.FRAGMENT | GPUShaderStage.COMPUTE;
        const layout = (entries: [GPUShaderStageFlags, Omit<GPUBindGroupLayoutEntry, 'binding' | 'visibility'>][]) =>
            device.createBindGroupLayout({ entries: entries.map(([visibility, entry], binding) => ({ binding, visibility, ...entry })) });
        const read: GPUBufferBindingLayout = { type: 'read-only-storage' }, write: GPUBufferBindingLayout = { type: 'storage' };
        const surfaceLayout = layout([
            [all, { buffer: { type: 'uniform' } }], [all, { buffer: read }], [all, { texture: { sampleType: 'uint', viewDimension: '2d-array' } }],
            [all, { buffer: read }], [all, { texture: { sampleType: 'float', viewDimension: '2d-array' } }], [all, { sampler: { type: 'non-filtering' } }],
        ]);
        const meshLayout = layout([[GPUShaderStage.VERTEX | GPUShaderStage.FRAGMENT, { buffer: { type: 'uniform' } }], [GPUShaderStage.VERTEX, { buffer: read }]]);
        const jobLayout = layout([[GPUShaderStage.COMPUTE, { buffer: read }], [GPUShaderStage.COMPUTE, { buffer: write }], [GPUShaderStage.COMPUTE, { buffer: write }]]);
        const quantizeLayout = layout([[GPUShaderStage.COMPUTE, { buffer: read }], [GPUShaderStage.COMPUTE, { buffer: read }], [GPUShaderStage.COMPUTE, { buffer: read }], [GPUShaderStage.COMPUTE, { buffer: write }], [GPUShaderStage.COMPUTE, { buffer: write }]]);
        const probeLayout = layout([[GPUShaderStage.COMPUTE, { buffer: read }], [GPUShaderStage.COMPUTE, { buffer: write }]]);
        const checkLayout = layout([[GPUShaderStage.VERTEX, { buffer: read }]]);
        const group = (layout: GPUBindGroupLayout, resources: GPUBindingResource[]) => device.createBindGroup({ layout, entries: resources.map((resource, binding) => ({ binding, resource })) });
        const pipelineLayout = (...groups: GPUBindGroupLayout[]) => device.createPipelineLayout({ bindGroupLayouts: groups });

        const swap: Record<string, string> = device.features.has('clip-distances') ? { 'terrain/unknown.wgsl': 'terrain/unknown-clip.wgsl' } : {};
        const modules = Object.fromEntries(['terrain', 'pages', 'probe', 'quantize'].map(name => [name, device.createShaderModule({ label: name, code: wgsl(`terrain/${name}.wgsl`, swap) })]));
        const compute = (module: string, entryPoint: string, layout: GPUPipelineLayout, constants: Record<string, number>) =>
            device.createComputePipelineAsync({ label: entryPoint, layout, compute: { module: modules[module], entryPoint, constants } });
        const [mesh, synthesize, normalsOf, quantize, probe, check] = await Promise.all([
            device.createRenderPipelineAsync({
                label: 'terrain', layout: pipelineLayout(meshLayout, surfaceLayout),
                vertex: { module: modules.terrain, entryPoint: 'vertex_main', constants },
                fragment: { module: modules.terrain, entryPoint: 'fragment_main', constants, targets: [{ format: gpu.format }] },
                primitive: { topology: 'triangle-list' },
                depthStencil: { format: depthFormat, depthWriteEnabled: true, depthCompare: 'greater' },
            }),
            compute('pages', 'synthesize', pipelineLayout(jobLayout, surfaceLayout), { ...constants, EROSION: labs.relief === 'erosion' ? 1 : 0 }),
            compute('pages', 'normals_of', pipelineLayout(jobLayout, surfaceLayout), constants),
            compute('quantize', 'quantize', pipelineLayout(quantizeLayout), {}),
            compute('probe', 'probe', pipelineLayout(probeLayout, surfaceLayout), constants),
            device.createRenderPipelineAsync({
                label: 'check', layout: pipelineLayout(checkLayout, surfaceLayout),
                vertex: { module: modules.probe, entryPoint: 'check_vertex', constants },
                fragment: { module: modules.probe, entryPoint: 'check_fragment', constants, targets: [{ format: 'r32float' }] },
                primitive: { topology: 'point-list' },
            }),
        ]);
        this.resources = {
            ...r, mesh, synthesize, normalsOf, quantize, probe, check, probeLayout, checkLayout,
            surfaceGroup: group(surfaceLayout, [{ buffer: r.surface }, { buffer: r.map }, heights.createView({ dimension: '2d-array' }), { buffer: r.scales }, normals.createView({ dimension: '2d-array' }), device.createSampler()]),
            meshGroup: group(meshLayout, [{ buffer: frameUniform }, { buffer: r.nodes }]),
            synthGroup: group(jobLayout, [{ buffer: r.synthJobs }, { buffer: r.synthOut }, { buffer: r.extent }]),
            normalGroup: group(jobLayout, [{ buffer: r.normalJobs }, { buffer: r.normalOut }, { buffer: r.extent }]),
            quantizeGroup: group(quantizeLayout, [{ buffer: r.synthJobs }, { buffer: r.synthOut }, { buffer: r.extent }, { buffer: r.codes }, { buffer: r.scales }]),
            probeGroup: group(probeLayout, [{ buffer: r.points }, { buffer: r.probed }]),
        };
    }

    /**
     * Selects the nodes for a view and takes the pages that they want. A frame that wants more pages
     * than a pool holds draws coarser from the next frame on: quality goes down before memory goes up.
     */
    update(view: View): void {
        const r = this.resources;
        if (!r) return;
        this.frameNumber++;
        const anchor = anchorAt(...view.lonLat), eye = { anchor, above: view.above };
        const rule = this.rule(eye, view.pixels);
        const forward = renderMap(this.frame, anchor);
        const start = performance.now();
        const { nodes, wanted } = select(rule, this.bounds, node => this.visible(node, eye, forward, view));
        this.selection = performance.now() - start;
        const ex = anchor.whole[0] + anchor.part[0], ey = anchor.whole[1] + anchor.part[1];
        const far = (node: Node) => Math.hypot((node.x + 0.5) * size(node.level) - ex, (node.y + 0.5) * size(node.level) - ey) / size(node.level);
        wanted.sort((a, b) => a.level - b.level || far(a) - far(b));
        this.nodes = nodes.slice(0, MAX_NODES);
        const work = this.pages.update(wanted, anchor);
        const { capacity, wanted: counts } = this.pages;
        if (this.pages.shortage > 0) {
            this.quality++;
            this.calm = 0;
        } else if (++this.calm > CALM && this.quality > 0 && counts.stored < ROOM * capacity.stored && counts.synth < ROOM * capacity.synth) {
            this.quality--;
            this.calm = 0;
        }
        if (work.map || work.uploads.length || work.syntheses.length) this.changes++;
        this.work = work;
        this.anchor = anchor;
        this.above = view.above;
        this.lonLat = view.lonLat;
        this.write(r, anchor, rule, forward, work, view.above);
    }

    /**
     * Encodes this frame's compute: the heights of synthesized pages, the normals, and the probe under
     * the eye when the view or the pages changed. Call the returned function after the submit.
     */
    encode(encoder: GPUCommandEncoder): () => void {
        const r = this.resources, work = this.work;
        if (!r || !work) return () => {};
        const r16 = this.labs.pages === 'r16', texels = PAGE * PAGE;
        if (work.syntheses.length) {
            const pass = encoder.beginComputePass({ label: 'synthesis' });
            pass.setPipeline(r.synthesize);
            pass.setBindGroup(0, r.synthGroup);
            pass.setBindGroup(1, r.surfaceGroup);
            pass.dispatchWorkgroups(PAGE / 8, PAGE / 8, work.syntheses.length);
            pass.end();
            if (r16) {
                const quantize = encoder.beginComputePass({ label: 'quantize' });
                quantize.setPipeline(r.quantize);
                quantize.setBindGroup(0, r.quantizeGroup);
                quantize.dispatchWorkgroups(PAGE / 16, PAGE / 8, work.syntheses.length);
                quantize.end();
            }
            work.syntheses.forEach(({ page }, k) => encoder.copyBufferToTexture(
                r16 ? { buffer: r.codes, offset: k * texels * 2, bytesPerRow: PAGE * 2 } : { buffer: r.synthOut, offset: k * texels * 4, bytesPerRow: PAGE * 4 },
                { texture: r.heights, origin: origin(page.slot) }, [PAGE, PAGE, 1]));
        }
        if (work.normals.length) {
            const pass = encoder.beginComputePass({ label: 'normals' });
            pass.setPipeline(r.normalsOf);
            pass.setBindGroup(0, r.normalGroup);
            pass.setBindGroup(1, r.surfaceGroup);
            pass.dispatchWorkgroups(PAGE / 16, PAGE / 8, work.normals.length);
            pass.end();
            work.normals.forEach((page, k) => encoder.copyBufferToTexture(
                { buffer: r.normalOut, offset: k * texels * 2, bytesPerRow: PAGE * 2 }, { texture: r.normals, origin: origin(page.slot) }, [PAGE, PAGE, 1]));
        }
        const state = `${this.anchor?.whole}/${this.anchor?.part}/${this.above}/${this.quality}/${this.changes}`;
        const readback = state === this.probed ? undefined : r.readbacks.pop();
        if (!readback) return () => {};
        this.probed = state;
        const probe = encoder.beginComputePass({ label: 'probe' });
        probe.setPipeline(r.probe);
        probe.setBindGroup(0, r.probeGroup);
        probe.setBindGroup(1, r.surfaceGroup);
        probe.dispatchWorkgroups(1);
        probe.end();
        encoder.copyBufferToBuffer(r.probed, 0, readback, 0, 20);
        const frame = this.frameNumber, lonLat = this.lonLat;
        this.probing++;
        return () => readback.mapAsync(GPUMapMode.READ).then(() => {
            const [height, east, west, north, south] = new Float32Array(readback.getMappedRange());
            readback.unmap();
            r.readbacks.push(readback);
            const known = (h: number) => h > -1e29, step = 2 * PROBE_STEP;
            const slope: [number, number] = [east, west, north, south].every(known) ? [(east - west) / step, (north - south) / step] : [0, 0];
            if (frame > this.floor && (!this.ground || frame > this.ground.frame)) this.ground = { height: known(height) ? height : NaN, slope, lonLat, frame };
        }, () => {}).finally(() => {
            this.probing--;
            this.changed();
        });
    }

    draw(pass: GPURenderPassEncoder): void {
        const r = this.resources;
        if (!r || !this.nodes.length) return;
        pass.setPipeline(r.mesh);
        pass.setBindGroup(0, r.meshGroup);
        pass.setBindGroup(1, r.surfaceGroup);
        pass.setIndexBuffer(r.indices, 'uint16');
        pass.drawIndexed(GRID_INDICES.length, this.nodes.length);
    }

    /**
     * drawnHeight at points, from the probe's compute shader and from a vertex shader as an object
     * instance has it, with the state of the last frame.
     */
    async check(points: [number, number][]): Promise<{ probe: number[]; instance: number[] }> {
        const r = this.resources, anchor = this.anchor;
        if (!r || !anchor || points.length > CHECKS) throw new Error(`A check takes up to ${CHECKS} points after the first frame.`);
        const { device } = r.gpu, n = points.length, row = 256;
        const at = new Float32Array(points.flatMap(([lon, lat]) => lodSpace(anchor, ...worldPixel([lon, lat], UNITS))));
        const input = device.createBuffer({ size: at.byteLength, usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST });
        const output = device.createBuffer({ size: 4 * n, usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC });
        const target = device.createTexture({ size: [n, 1], format: 'r32float', usage: GPUTextureUsage.RENDER_ATTACHMENT | GPUTextureUsage.COPY_SRC });
        const read = [4 * n, row].map(size => device.createBuffer({ size, usage: GPUBufferUsage.MAP_READ | GPUBufferUsage.COPY_DST }));
        device.queue.writeBuffer(input, 0, at);
        const encoder = device.createCommandEncoder();
        const compute = encoder.beginComputePass();
        compute.setPipeline(r.probe);
        compute.setBindGroup(0, device.createBindGroup({ layout: r.probeLayout, entries: [{ binding: 0, resource: { buffer: input } }, { binding: 1, resource: { buffer: output } }] }));
        compute.setBindGroup(1, r.surfaceGroup);
        compute.dispatchWorkgroups(1);
        compute.end();
        const render = encoder.beginRenderPass({ colorAttachments: [{ view: target.createView(), loadOp: 'clear', storeOp: 'store', clearValue: [0, 0, 0, 0] }] });
        render.setPipeline(r.check);
        render.setBindGroup(0, device.createBindGroup({ layout: r.checkLayout, entries: [{ binding: 0, resource: { buffer: input } }] }));
        render.setBindGroup(1, r.surfaceGroup);
        render.draw(n);
        render.end();
        encoder.copyBufferToBuffer(output, 0, read[0], 0, 4 * n);
        encoder.copyTextureToBuffer({ texture: target }, { buffer: read[1], bytesPerRow: row }, [n, 1]);
        device.queue.submit([encoder.finish()]);
        const [probe, instance] = await Promise.all(read.map(async buffer => {
            await buffer.mapAsync(GPUMapMode.READ);
            return Array.from(new Float32Array(buffer.getMappedRange(), 0, n), h => (h > -1e29 ? h : NaN));
        }));
        for (const resource of [input, output, target, ...read]) resource.destroy();
        return { probe, instance };
    }

    private rule(eye: { anchor: Anchor; above: number }, pixels: number): Rule {
        if (this.labs.terrain === 'rings') return rings(eye, RINGS[Math.min(this.quality, RINGS.length - 1)], FINEST - Math.max(0, this.quality - RINGS.length + 1));
        // The range at which a level's grid cell spans CELL_PIXELS, a quarter step smaller per quality step.
        const best = Math.min(RANGE.most, Math.max(RANGE.least, pixels / (2 * Math.tan(VFOV * RAD / 2)) / (GRID * CELL_PIXELS)));
        const steps = Math.ceil(4 * Math.log2(best / RANGE.least));
        return cdlod(eye, Math.max(RANGE.least, best * 2 ** (-this.quality / 4)), FINEST - Math.max(0, this.quality - steps));
    }

    /** Whether a node's box can show: render space corners at its range of heights, against the frustum. */
    private visible(node: Node, eye: { anchor: Anchor; above: number }, forward: Float64Array, view: View): boolean {
        if (view.z === undefined) return true;
        const [lo, hi] = this.pages.range(node) ?? ANY_HEIGHT;
        const [near, far] = distances({ ...eye, above: 0 }, node).map(d => d * d * CURVATURE);
        const s = size(node.level), [x0, y0] = lodSpace(eye.anchor, node.x * s, node.y * s), [x1, y1] = lodSpace(eye.anchor, (node.x + 1) * s, (node.y + 1) * s);
        const points: number[][] = [];
        for (const fx of [0, 0.5, 1]) for (const fy of [0, 0.5, 1]) {
            const sx = x0 + (x1 - x0) * fx, sy = y0 + (y1 - y0) * fy, rx = cubic(forward, sx, sy, 0), ry = cubic(forward, sx, sy, 10);
            points.push([rx, ry, lo - MARGIN - view.z - far], [rx, ry, hi + MARGIN - view.z - near]);
        }
        const m = view.viewProj, row = (i: number) => [m[i], m[4 + i], m[8 + i], m[12 + i]];
        const planes = [[3, 0, 1], [3, 0, -1], [3, 1, 1], [3, 1, -1], [3, 2, -1]].map(([w, axis, sign]) => row(w).map((v, k) => v + sign * row(axis)[k]));
        return planes.every(plane => points.some(([x, y, z]) => plane[0] * x + plane[1] * y + plane[2] * z + plane[3] >= 0));
    }

    private write(r: Resources, anchor: Anchor, rule: Rule, forward: Float64Array, work: Work, above: number): void {
        const { device } = r.gpu, r16 = this.labs.pages === 'r16';
        const surface = new ArrayBuffer(SURFACE_BYTES), ints = new Int32Array(surface), floats = new Float32Array(surface);
        ints.set(anchor.whole, 0);
        floats.set([...anchor.part, anchor.unit, above, rule.range], 2);
        new Uint32Array(surface)[7] = rule.finest;
        floats.set(forward, 8);
        work.origins.forEach((origin, level) => {
            const centre = level > 1 ? rule.centre(level) : [anchor.whole[0], anchor.whole[1]];
            ints.set([...origin, centre[0] - anchor.whole[0], centre[1] - anchor.whole[1]], 28 + 4 * level);
        });
        device.queue.writeBuffer(r.surface, 0, surface);
        // The points of the probe in LOD space, by the linear part of the cubic at the eye.
        const [a, b, c, d] = [forward[1], forward[2], forward[11], forward[12]], k = PROBE_STEP / (a * d - b * c);
        device.queue.writeBuffer(r.points, 0, new Float32Array([0, 0, d * k, -c * k, -d * k, c * k, -b * k, a * k, b * k, -a * k]));
        if (work.map) device.queue.writeBuffer(r.map, 0, work.map);
        for (const { page, data } of work.uploads) {
            device.queue.writeTexture({ texture: r.heights, origin: origin(page.slot) }, data.texels, { bytesPerRow: PAGE * data.texels.BYTES_PER_ELEMENT }, [PAGE, PAGE, 1]);
            if (r16) device.queue.writeBuffer(r.scales, 8 * page.slot, new Float32Array([data.offset, data.scale]));
        }
        device.queue.writeBuffer(r.synthJobs, 0, jobs(work.syntheses.map(({ page, stored }) => [page, stored])));
        device.queue.writeBuffer(r.normalJobs, 0, jobs(work.normals.map(page => [page, 0])));
        if (r16) device.queue.writeBuffer(r.extent, 0, new Uint32Array(2 * ARRIVALS).map((_, i) => (i % 2 ? 0 : 0xffffffff)));
        const instances = new Int32Array(4 * this.nodes.length);
        this.nodes.forEach(({ level, x, y }, i) => instances.set([x * size(level) - anchor.whole[0], y * size(level) - anchor.whole[1], level, 0], 4 * i));
        device.queue.writeBuffer(r.nodes, 0, instances);
    }
}

/** Where a page lies: one of 8 × 8 tiles of a layer, as `in_layer` in drawn-height.wgsl has it. */
const origin = (slot: number): [number, number, number] => [PAGE * (slot & 7), PAGE * ((slot >> 3) & 7), slot >> 6];

/** Jobs as `Job` in the WGSL: the node's first texel, its level, the stored level, metres per texel and the slot. */
function jobs(list: [Page, number][]): ArrayBuffer {
    const data = new ArrayBuffer(Math.max(1, list.length) * JOB_BYTES), ints = new Int32Array(data), floats = new Float32Array(data);
    list.forEach(([{ node: { level, x, y }, slot }, stored], k) => {
        const lat = tileLonLat(x + 0.5, y + 0.5, level)[1];
        ints.set([x * PAGE, y * PAGE, level, stored], 8 * k);
        floats[8 * k + 4] = 2 * Math.PI * EARTH_RADIUS * Math.cos(lat * RAD) / 2 ** (level + 8);
        ints[8 * k + 5] = slot;
    });
    return data;
}

/** The triangles of the grid mesh, with vertex j × 65 + i at cell column i and row j. */
function gridIndices(): Uint16Array<ArrayBuffer> {
    const out = new Uint16Array(6 * GRID * GRID), n = GRID + 1;
    let k = 0;
    for (let j = 0; j < GRID; j++) for (let i = 0; i < GRID; i++) {
        const a = j * n + i;
        out.set([a, a + n, a + 1, a + 1, a + n, a + n + 1], k);
        k += 6;
    }
    return out;
}
