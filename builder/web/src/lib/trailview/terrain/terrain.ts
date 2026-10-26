import type { Bounds } from '../../planner/config';
import { worldPixel } from '../../planner/layers/mercator';
import { VFOV } from '../frame/camera';
import { CURVATURE, type Frame } from '../frame/frame';
import { anchorAt, cubic, lodSpace, renderMap, UNITS, type Anchor } from '../frame/space';
import type { Gpu } from '../gpu/device';
import { CLIP_DISTANCES } from '../gpu/include';
import { layouts } from '../gpu/layouts';
import { wgsl } from '../gpu/wgsl';
import { PAGE } from '../stream/heights';
import type { TerrainSource } from '../stream/terrain';
import { DEPTH, sceneFormat } from '../resolve/targets';
import { featureTile, NEAR, profileTable } from './bins';
import { cdlod, distances, FINEST, GRID, key, MAP, NEAR_M, RANGE, select, size, type Node, type Rule } from './lod';
import { ARRIVALS, copies, MODE, NO_LIMIT, Pages, STAMP_WORDS, type Kind, type Page, type Stamp, type Work } from './pages';
import { Stamps, type FeatureTiles } from './stamps';
import { Water } from './water';
import { Structures } from '../structures/structures';
import { BOX } from '../structures/node';

/**
 * GPU memory for pages: the stored and the synthesized budgets for a laptop, in MiB. The feature masks
 * take their share of the synthesized budget.
 */
const BUDGET: Record<Kind, number> = { stored: 120 * 2 ** 20, synth: 51 * 2 ** 20 };
/**
 * Feature masks of the near pages, 4 bytes per texel: 32 MiB of the synthesized budget. The views of
 * the test box draw up to 106 near pages with features, and every one of them needs its mask: a
 * page without one draws its roads unpainted.
 */
const MASKS = 128, MASK_BYTES = PAGE * PAGE * MASKS * 4;
/** Per mask, the snaps of its page's 64 × 64 mesh vertices, 4 bytes each, as `snaps` in features.wgsl. */
const SNAP_BYTES = (PAGE / 4) ** 2 * 4;
/** Portal boxes in the terrain's holes at most. */
const MAX_HOLES = 32;
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
const SURFACE_BYTES = 416, JOB_BYTES = 24, STAMP_JOB_BYTES = 32, NODE_BYTES = 16;
/** A stamp job whose base is the synthesized page in `out`, as SYNTHESIZED in stamp.wgsl. */
const SYNTHESIZED = 0xffffffff;
/** Audit samples per run at most, words per sample, and words of results, as in audit.wgsl. */
const SAMPLES = 2 ** 18, SAMPLE_WORDS = 12, COUNTS = 45, FOUND = 64;
/** Cells per strip of the grid's index order. */
const STRIP = 8;
const GRID_INDICES = gridIndices();
/** The road and path profiles, as stamp-core.wgsl reads them. */
const PROFILE_TABLE = profileTable();

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
    /** The stamp jobs of the last frame: heights on near pages, heights on coarser pages (lakes), and masks only. */
    jobs: { near: number; coarse: number; masks: number };
    /** Masks in use. */
    masks: number;
    /** Water and structure meshes of the last frame that did not fit in their pools. */
    waterDropped: number;
    structuresDropped: number;
}

/** What the audit counts, as audit.wgsl does. */
export interface Audit {
    /**
     * Per check: samples checked, violations, misses in a conflict, misses within the reach of a junction,
     * where the simple profiles of the bake meet, and the largest error elsewhere and at a junction in metres.
     */
    checks: Record<'crossFall' | 'roadHeight' | 'inWater' | 'onBank' | 'shoreSide' | 'underDeck', { checked: number; violations: number; conflicts: number; junctions: number; largest: number; junctionLargest: number }>;
    /** Lakes of the frame's feature tiles, and those with more than one level. */
    levels: { lakes: number; mixed: number };
    /** Rivers of the frame's feature tiles, and those whose level rises downstream. */
    rivers: { checked: number; rising: number };
    apron: { checked: number; differ: number };
    /** The first violations: kind, level, the point in LOD space, drawnHeight and the expected height. */
    found: { kind: number; level: number; s: [number, number]; drawn: number; expected: number; detail: number }[];
}

interface Resources extends Pipelines {
    gpu: Gpu;
    heights: GPUTexture;
    mask: GPUTexture;
    masks: GPUBuffer;
    snaps: GPUTexture;
    stampJobs: GPUBuffer;
    stampData: GPUBuffer;
    profiles: GPUBuffer;
    surface: GPUBuffer;
    map: GPUBuffer;
    scales: GPUBuffer;
    nodes: GPUBuffer;
    holes: GPUBuffer;
    synthJobs: GPUBuffer;
    synthOut: GPUBuffer;
    extent: GPUBuffer;
    codes: GPUBuffer;
    indices: GPUBuffer;
    points: GPUBuffer;
    probed: GPUBuffer;
    readbacks: GPUBuffer[];
    surfaceGroup: GPUBindGroup;
    meshGroup: GPUBindGroup;
    synthGroup: GPUBindGroup;
    quantizeGroup: GPUBindGroup;
    probeGroup: GPUBindGroup;
    stampGroup: GPUBindGroup;
    snapGroup: GPUBindGroup;
    featureGroup: GPUBindGroup;
    emptyGroup: GPUBindGroup;
    /** The draw of the nodes with a tunnel's portal, after `attachParts`: until then they draw without their hole. */
    holedMesh?: GPURenderPipeline;
}

/**
 * The terrain: CDLOD by distance over pages of the stored terrain and of
 * levels synthesized below it, in fixed pools. One instanced grid mesh draws every node.
 */
export class Terrain {
    readonly pages: Pages;
    readonly stamps: Stamps;
    /** Whether the last frame draws other content than the frame before it: new pages, masks, nodes or water. */
    newContent = false;
    /** The metres on the ground from the eye to the nearest page whose heights the last frame changed, 0 for new structures: casters stand on them. */
    reshaped = Infinity;
    private readonly water: Water;
    private readonly structures: Structures;
    /**
     * The probed ground under the eye at `lonLat`: NaN where no level knows it, a slope of 0 where no
     * level knows a point about it, and the frame that it comes from.
     */
    ground?: { height: number; slope: [number, number]; lonLat: [number, number]; frame: number };
    private resources?: Resources;
    private readonly bounds: [number, number, number, number];
    private nodes: Node[] = [];
    /** The first drawn nodes that hold a tunnel's portal: they draw with its hole. */
    private holed = 0;
    /** The portals' boxes of those nodes, as BOX words per box. */
    private boxes: Float64Array[] = [];
    private wantedNodes: Node[] = [];
    private work?: Work;
    private jobs = { near: 0, coarse: 0, masks: 0 };
    private anchor?: Anchor;
    private quality = 0;
    private calm = 0;
    private selection = 0;
    private frameNumber = 0;
    private probing = 0;
    /** Whether each selected node within NEAR_M drew its own page in the last frame, with its road paint. */
    private nearDrawn = false;
    /**
     * The state that the last probe read, a count of the frames that changed the pages or the masks,
     * which move the snapped mesh, and the last of those frames.
     */
    private probed = '';
    private changes = 0;
    private changedAt = 0;
    private above = 0;
    private lonLat: [number, number] = [0, 0];
    /** The camera of the last frame, for the audit: only what the frame shows counts. */
    private seen?: { viewProj: Float32Array; z: number; forward: Float64Array };
    private floor = 0;
    private instances = new Int32Array(0);

    constructor(private readonly frame: Frame, source: TerrainSource, features: FeatureTiles | undefined, bounds: Bounds, private readonly changed: () => void) {
        const [west, north] = worldPixel([bounds[0], bounds[3]], UNITS), [east, south] = worldPixel([bounds[2], bounds[1]], UNITS);
        this.bounds = [west, north, east, south];
        // 16-bit heights.
        const bytes = PAGE * PAGE * 2;
        // Whole layers inside the two budgets; the synthesized pool takes its share, the stored pool the rest.
        const pages = Math.floor((BUDGET.stored + BUDGET.synth - MASK_BYTES) / bytes / LAYER_PAGES) * LAYER_PAGES, synth = Math.floor((BUDGET.synth - MASK_BYTES) / bytes);
        this.stamps = new Stamps(features, changed);
        this.water = new Water(this.stamps);
        this.structures = new Structures(this.stamps);
        this.pages = new Pages(source, { stored: pages - synth, synth }, this.bounds, changed, this.stamps, MASKS);
    }

    get idle(): boolean {
        return this.pages.idle && this.calm > 0 && this.probing === 0;
    }

    /**
     * Whether the near field draws its final heights: each selected node within NEAR_M draws its own
     * page with its stamps and masks, and the probe under the eye ran after the last change of the pages.
     */
    get near(): boolean {
        return this.nearDrawn && !!this.ground && this.ground.frame >= this.changedAt;
    }

    /** Whether each drawn node of `near` has the road paint of its mask where the mask pool reaches it. */
    private painted(nodes: Node[], near: Set<string>, anchor: Anchor): boolean {
        return nodes.every(node => {
            const page = this.pages.pages.get(key(node));
            return !near.has(key(node)) || page?.drawn === undefined || node.level < NEAR || page.mask !== undefined || !this.stamps.known(node)?.data
                || distances({ anchor, above: 0 }, node)[0] >= this.pages.maskReach;
        });
    }

    /** True while the terrain needs frames that no tile load, feature tile or probe will ask for. */
    get working(): boolean {
        // A frame that changed nothing while feature tiles are on their way waits for them.
        return (!this.pages.idle && this.pages.loads === 0 && (this.newContent || !this.stamps.loading)) || this.calm === 0;
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
        let masks = 0;
        for (const page of this.pages.pages.values()) {
            if (page.ready) resident[page.kind] += page.copy === undefined ? 1 : 2;
            if (page.mask !== undefined) masks++;
        }
        return {
            nodes: this.nodes.length, triangles: 2 * GRID * GRID * this.nodes.length, selection: this.selection, resident, capacity: { ...this.pages.capacity }, quality: this.quality,
            jobs: this.jobs, masks, waterDropped: this.water.dropped, structuresDropped: this.structures.dropped,
        };
    }

    /** The nodes that the last frame drew and wanted, the nodes whose drawn heights it changed, and the LOD map windows. */
    get selected(): { drawn: Node[]; wanted: Node[]; arrived: Node[]; origins: [number, number][] } {
        return { drawn: this.nodes, wanted: this.wantedNodes, arrived: this.work?.fresh.map(page => page.node) ?? [], origins: this.work?.origins ?? [] };
    }

    /** Encodes the heights of the structures' anchors into a compute pass of the frame. */
    encodeAnchors(pass: GPUComputePassEncoder): void {
        const r = this.resources;
        if (r) this.structures.encode(pass, r.surfaceGroup, r.featureGroup);
    }

    /** The models on the surface of the kinds of the drawn nodes, a list per node. */
    get kindPoints(): Uint32Array[] {
        return this.structures.points;
    }

    /** The highest floor of a deck or a tunnel at a point between heights `low` and `high`, where it is known. */
    floorAt(lonLat: [number, number], low: number, high: number): number | undefined {
        return this.structures.floor(...worldPixel(lonLat, UNITS), low, high);
    }

    /** The drawn surface's pages for other passes: group 1 of the terrain. drawn_height reads the feature mask of group 3 too. */
    get surface(): GPUBindGroup | undefined {
        return this.resources?.surfaceGroup;
    }

    /** The feature mask for other passes: group 3 of the terrain, which drawn_height reads. */
    get features(): GPUBindGroup | undefined {
        return this.resources?.featureGroup;
    }

    /**
     * Takes the pools and the pipelines of the first frame on a device. A new device gets every page
     * again, from the kept tile bodies.
     */
    async attach(gpu: Gpu, frameUniform: GPUBuffer): Promise<void> {
        this.resources = undefined;
        this.pages.lost();
        const { device } = gpu, pages = this.pages.capacity.stored + this.pages.capacity.synth;
        const layers = Math.ceil(pages / LAYER_PAGES), side = PAGE * Math.sqrt(LAYER_PAGES);
        const texels = PAGE * PAGE;
        const usage = GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST;
        const heights = gpu.texture('pages', { size: [side, side, layers], format: 'r16uint', usage }, layers * side * side * 2);
        // The masks: written by the stamp pass, read by the shaders that draw.
        const mask = gpu.texture('features', {
            size: [side, Math.ceil(Math.min(MASKS, LAYER_PAGES) / 8) * PAGE, Math.ceil(MASKS / LAYER_PAGES)], format: 'rgba8unorm', usage: GPUTextureUsage.STORAGE_BINDING | GPUTextureUsage.TEXTURE_BINDING,
        }, MASK_BYTES);
        const snaps = gpu.texture('features', {
            size: [PAGE / 4, PAGE / 4, MASKS], format: 'r32uint', usage: GPUTextureUsage.STORAGE_BINDING | GPUTextureUsage.TEXTURE_BINDING,
        }, MASKS * SNAP_BYTES);
        const storage = GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST;
        const buffer = (pool: string, bytes: number, usage: number) => gpu.buffer(pool, bytes, usage);
        const r: Omit<Resources, keyof Pipelines | 'holedMesh' | 'surfaceGroup' | 'meshGroup' | 'synthGroup' | 'quantizeGroup' | 'probeGroup' | 'stampGroup' | 'snapGroup' | 'featureGroup' | 'emptyGroup'> = {
            gpu, heights,
            mask, snaps,
            masks: buffer('features', (1 + pages) * 4, storage),
            stampJobs: buffer('compute', ARRIVALS * STAMP_JOB_BYTES, storage),
            stampData: buffer('compute', STAMP_WORDS * 4, storage),
            profiles: buffer('compute', PROFILE_TABLE.byteLength, GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST),
            surface: buffer('terrain', SURFACE_BYTES, GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST),
            map: buffer('terrain', (FINEST + 1) * MAP * MAP * 4, storage),
            scales: buffer('terrain', pages * 8, storage),
            nodes: buffer('terrain', MAX_NODES * NODE_BYTES, storage),
            holes: buffer('terrain', (1 + 3 * MAX_HOLES) * 16, storage),
            synthJobs: buffer('compute', ARRIVALS * JOB_BYTES, storage),
            synthOut: buffer('compute', ARRIVALS * texels * 4, GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC),
            extent: buffer('compute', ARRIVALS * 8, storage),
            codes: buffer('compute', ARRIVALS * texels * 2, GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC),
            indices: buffer('terrain', GRID_INDICES.byteLength, GPUBufferUsage.INDEX | GPUBufferUsage.COPY_DST),
            // The eye and four points about it, for the slope.
            points: buffer('probe', 40, storage),
            probed: buffer('probe', 20, GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC),
            readbacks: Array.from({ length: READBACKS }, () => buffer('probe', 20, GPUBufferUsage.MAP_READ | GPUBufferUsage.COPY_DST)),
        };
        device.queue.writeBuffer(r.indices, 0, GRID_INDICES);
        device.queue.writeBuffer(r.profiles, 0, PROFILE_TABLE);
        device.queue.writeBuffer(r.scales, 0, new Float32Array(2 * pages).fill(1));

        const L = gpu.once(terrainLayouts), { surface: surfaceLayout, features: featureLayout } = gpu.once(layouts), pipelines = await gpu.once(terrainPipelines);
        const group = (layout: GPUBindGroupLayout, resources: GPUBindingResource[]) => device.createBindGroup({ layout, entries: resources.map((resource, binding) => ({ binding, resource })) });
        const view = (texture: GPUTexture) => texture.createView({ dimension: '2d-array' });
        // No mask yet, and a reach of the paint that no view meets.
        device.queue.writeBuffer(r.masks, 0, new Float32Array(1 + pages).fill(NO_LIMIT, 0, 1));
        this.resources = {
            ...r, ...pipelines,
            stampGroup: group(L.stamp, [{ buffer: r.stampJobs }, { buffer: r.stampData }, { buffer: r.synthOut }, { buffer: r.extent }, { buffer: r.profiles }, view(r.mask)]),
            snapGroup: device.createBindGroup({
                layout: L.snap, entries: [{ binding: 0, resource: { buffer: r.stampJobs } }, { binding: 6, resource: view(r.mask) }, { binding: 7, resource: view(r.snaps) }],
            }),
            featureGroup: group(featureLayout, [{ buffer: r.masks }, view(r.mask), view(r.snaps)]),
            emptyGroup: group(L.empty, []),
            surfaceGroup: group(surfaceLayout, [{ buffer: r.surface }, { buffer: r.map }, heights.createView({ dimension: '2d-array' }), { buffer: r.scales }, device.createSampler()]),
            meshGroup: group(L.mesh, [{ buffer: frameUniform }, { buffer: r.nodes }, { buffer: r.holes }]),
            synthGroup: group(L.job, [{ buffer: r.synthJobs }, { buffer: r.synthOut }, { buffer: r.extent }]),
            quantizeGroup: group(L.quantize, [{ buffer: r.synthJobs }, { buffer: r.synthOut }, { buffer: r.extent }, { buffer: r.codes }, { buffer: r.scales }]),
            probeGroup: group(L.probe, [{ buffer: r.points }, { buffer: r.probed }]),
        };
    }

    /**
     * Takes the water, the structures and the holes of the tunnels' portals on the device of `attach`:
     * they draw once compiled. `frames` are the frame uniforms of the eye's view and of the sun's, whose
     * casters draw into the shadow map.
     */
    async attachParts(frames: GPUBuffer[]): Promise<void> {
        const r = this.resources!;
        await Promise.all([
            this.water.attach(r.gpu, frames[0], r.nodes), this.structures.attach(r.gpu, frames),
            r.gpu.once(holedPipeline).then(pipeline => void (r.holedMesh = pipeline)),
        ]);
    }

    /**
     * Selects the nodes for a view and takes the pages that they want, and the pages of `reads`, which
     * the ground's relief reads. A frame that wants more pages than a pool holds draws coarser from the
     * next frame on: quality goes down before memory goes up.
     */
    update(view: View, reads: Node[] = []): void {
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
        const order = (a: Node, b: Node) => a.level - b.level || far(a) - far(b), keys = new Set(wanted.map(key));
        // The ground takes the selection alone: with its own reads it would want the ground of coarser and coarser pages.
        this.wantedNodes = wanted.sort(order);
        const nearField = new Set(nodes.filter(node => distances({ anchor, above: 0 }, node)[0] <= NEAR_M).map(key));
        const work = this.pages.update([...wanted, ...reads.filter(node => !keys.has(key(node)))].sort(order), anchor, nearField);
        // After the update, so that a page draws in the frame that it arrives in. The nodes of the levels
        // with masks come first: they draw with the road paint, those with a tunnel's portal first of all,
        // with its hole. Each pipeline draws near to far, so that a GPU that shades before the depth test
        // shades few hidden fragments.
        const drawn = nodes.slice(0, MAX_NODES).filter(node => this.pages.covers(node));
        const group = drawn.map(node => (node.level < NEAR ? 2 : this.stamps.known(node)?.portals ? 0 : 1)), near = drawn.map(node => distances(eye, node)[0]);
        this.boxes = drawn.flatMap((node, i) => (group[i] === 0 ? [this.stamps.known(node)!.portals!] : []));
        this.nodes = drawn.map((_, i) => i).sort((a, b) => group[a] - group[b] || near[a] - near[b]).map(i => drawn[i]);
        this.holed = group.filter(g => g === 0).length;
        const drawing = this.nodes.map(node => this.pages.drawing(node)!);
        const water = this.water.update(drawing);
        const structures = this.structures.update(this.nodes, { x: ex, y: ey, unit: anchor.unit });
        const { capacity, wanted: counts } = this.pages;
        if (this.pages.shortage > 0) {
            this.quality++;
            this.calm = 0;
        } else if (++this.calm > CALM && this.quality > 0 && counts.stored < ROOM * capacity.stored && counts.synth < ROOM * capacity.synth) {
            this.quality--;
            this.calm = 0;
        }
        if (work.map || work.masks || work.uploads.length || work.syntheses.length) {
            this.changes++;
            this.changedAt = this.frameNumber;
        }
        this.work = work;
        this.nearDrawn = this.pages.nearMissing === 0 && this.painted(nodes, nearField, anchor);
        this.jobs = { near: 0, coarse: 0, masks: 0 };
        for (const { page, mode } of work.stamps) this.jobs[!(mode & MODE.HEIGHTS) ? 'masks' : page.node.level >= NEAR ? 'near' : 'coarse']++;
        this.anchor = anchor;
        this.above = view.above;
        this.lonLat = view.lonLat;
        this.seen = view.z === undefined ? undefined : { viewProj: view.viewProj, z: view.z, forward };
        const level = { anchor, above: 0 }, pages = [...work.uploads, ...work.syntheses, ...work.stamps.filter(({ mode }) => mode & MODE.HEIGHTS)];
        this.reshaped = structures ? 0 : Math.min(Infinity, ...pages.map(({ page }) => distances(level, page.node)[0]));
        this.newContent = this.write(r, anchor, rule, forward, work, view.above) || water || structures || !!(work.map || work.masks || work.uploads.length || work.syntheses.length || work.stamps.length);
    }

    /**
     * Encodes this frame's compute: the heights of synthesized pages, the stamps, and the probe under
     * the eye when the view or the pages changed. Call the returned function after the submit.
     */
    encode(encoder: GPUCommandEncoder, time: (pass: string) => GPUComputePassTimestampWrites | undefined = () => undefined): () => void {
        const r = this.resources, work = this.work;
        if (!r || !work) return () => {};
        const texels = PAGE * PAGE;
        if (work.syntheses.length) {
            const pass = encoder.beginComputePass({ label: 'synthesis' });
            pass.setPipeline(r.synthesize);
            pass.setBindGroup(0, r.synthGroup);
            pass.setBindGroup(1, r.surfaceGroup);
            pass.dispatchWorkgroups(PAGE / 8, PAGE / 8, work.syntheses.length);
            pass.end();
        }
        if (work.stamps.length) {
            const pass = encoder.beginComputePass({ label: 'stamp', timestampWrites: time('stamp') });
            pass.setPipeline(r.stamp);
            pass.setBindGroup(0, r.stampGroup);
            pass.setBindGroup(1, r.surfaceGroup);
            pass.dispatchWorkgroups(PAGE / 8, PAGE / 8, work.stamps.length);
            // The snaps of the mesh vertices, from the masks just written: one vertex per 4 × 4 texels.
            pass.setPipeline(r.snap);
            pass.setBindGroup(0, r.snapGroup);
            pass.dispatchWorkgroups(PAGE / 32, PAGE / 32, work.stamps.length);
            pass.end();
        }
        // The heights of page k of `out` go to its layer: the syntheses, then the stamped copies of stored pages.
        const targets = [...work.syntheses.map(({ page }) => page.slot), ...copies(work).map(({ target }) => target)];
        if (targets.length) {
            const quantize = encoder.beginComputePass({ label: 'quantize' });
            quantize.setPipeline(r.quantize);
            quantize.setBindGroup(0, r.quantizeGroup);
            quantize.dispatchWorkgroups(PAGE / 16, PAGE / 8, targets.length);
            quantize.end();
            targets.forEach((slot, k) => encoder.copyBufferToTexture(
                { buffer: r.codes, offset: k * texels * 2, bytesPerRow: PAGE * 2 },
                { texture: r.heights, origin: origin(slot) }, [PAGE, PAGE, 1]));
        }
        const state = `${this.anchor?.whole}/${this.anchor?.part}/${this.above}/${this.quality}/${this.changes}`;
        const readback = state === this.probed ? undefined : r.readbacks.pop();
        if (!readback) return () => {};
        this.probed = state;
        const probe = encoder.beginComputePass({ label: 'probe' });
        probe.setPipeline(r.probe);
        probe.setBindGroup(0, r.probeGroup);
        probe.setBindGroup(1, r.surfaceGroup);
        probe.setBindGroup(2, r.emptyGroup);
        probe.setBindGroup(3, r.featureGroup);
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

    /** Draws the structures into the shadow map (light/shadow.ts) as casters. */
    cast(pass: GPURenderPassEncoder): void {
        const r = this.resources;
        if (r) this.structures.cast(pass, r.surfaceGroup, r.featureGroup);
    }

    draw(pass: GPURenderPassEncoder, ground: GPUBindGroup): void {
        const r = this.resources;
        if (!r || !this.nodes.length) return;
        const painted = this.nodes.filter(node => node.level >= NEAR).length, holed = this.holed;
        // Before the terrain: the terrain that they hide fails the depth test, also on a GPU that shades
        // before the depth test. The ribbons blend over the terrain.
        this.structures.draw(pass, r.surfaceGroup, ground, r.featureGroup);
        this.water.draw(pass, r.surfaceGroup, ground, r.featureGroup, false);
        pass.setBindGroup(0, r.meshGroup);
        pass.setBindGroup(1, r.surfaceGroup);
        pass.setBindGroup(2, ground);
        pass.setBindGroup(3, r.featureGroup);
        pass.setIndexBuffer(r.indices, 'uint16');
        pass.setPipeline(r.holedMesh ?? r.mesh);
        if (holed) pass.drawIndexed(GRID_INDICES.length, holed);
        pass.setPipeline(r.mesh);
        if (painted > holed) pass.drawIndexed(GRID_INDICES.length, painted - holed, 0, 0, holed);
        pass.setPipeline(r.farMesh);
        if (painted < this.nodes.length) pass.drawIndexed(GRID_INDICES.length, this.nodes.length - painted, 0, 0, painted);
        this.water.draw(pass, r.surfaceGroup, ground, r.featureGroup, true);
    }

    /**
     * drawnHeight at points, from the probe's compute shader and from a vertex shader as an object
     * instance has it, and the share of the sun there under the ground group `ground`, with the state
     * of the last frame.
     */
    async check(points: [number, number][], ground: GPUBindGroup): Promise<{ probe: number[]; instance: number[]; sun: number[] }> {
        const r = this.resources, anchor = this.anchor;
        if (!r || !anchor || points.length > CHECKS) throw new Error(`A check takes up to ${CHECKS} points after the first frame.`);
        const { device } = r.gpu, n = points.length, row = 256, L = r.gpu.once(terrainLayouts), [check, sunProbe] = await r.gpu.once(checkPipelines);
        const at = new Float32Array(points.flatMap(([lon, lat]) => lodSpace(anchor, ...worldPixel([lon, lat], UNITS))));
        const input = device.createBuffer({ size: at.byteLength, usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST });
        const [output, shares] = [0, 1].map(() => device.createBuffer({ size: 4 * n, usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC }));
        const target = device.createTexture({ size: [n, 1], format: 'r32float', usage: GPUTextureUsage.RENDER_ATTACHMENT | GPUTextureUsage.COPY_SRC });
        const read = [4 * n, row, 4 * n].map(size => device.createBuffer({ size, usage: GPUBufferUsage.MAP_READ | GPUBufferUsage.COPY_DST }));
        device.queue.writeBuffer(input, 0, at);
        const encoder = device.createCommandEncoder();
        const compute = encoder.beginComputePass();
        compute.setPipeline(r.probe);
        compute.setBindGroup(0, device.createBindGroup({ layout: L.probe, entries: [{ binding: 0, resource: { buffer: input } }, { binding: 1, resource: { buffer: output } }] }));
        compute.setBindGroup(1, r.surfaceGroup);
        compute.setBindGroup(2, r.emptyGroup);
        compute.setBindGroup(3, r.featureGroup);
        compute.dispatchWorkgroups(1);
        compute.setPipeline(sunProbe);
        compute.setBindGroup(0, device.createBindGroup({ layout: L.probe, entries: [{ binding: 0, resource: { buffer: input } }, { binding: 1, resource: { buffer: shares } }] }));
        compute.setBindGroup(2, ground);
        compute.dispatchWorkgroups(1);
        compute.end();
        const render = encoder.beginRenderPass({ colorAttachments: [{ view: target.createView(), loadOp: 'clear', storeOp: 'store', clearValue: [0, 0, 0, 0] }] });
        render.setPipeline(check);
        render.setBindGroup(0, device.createBindGroup({ layout: L.check, entries: [{ binding: 0, resource: { buffer: input } }] }));
        render.setBindGroup(1, r.surfaceGroup);
        render.setBindGroup(2, r.emptyGroup);
        render.setBindGroup(3, r.featureGroup);
        render.draw(n);
        render.end();
        encoder.copyBufferToBuffer(output, 0, read[0], 0, 4 * n);
        encoder.copyTextureToBuffer({ texture: target }, { buffer: read[1], bytesPerRow: row }, [n, 1]);
        encoder.copyBufferToBuffer(shares, 0, read[2], 0, 4 * n);
        device.queue.submit([encoder.finish()]);
        const [probe, instance, sun] = await Promise.all(read.map(async buffer => {
            await buffer.mapAsync(GPUMapMode.READ);
            return Array.from(new Float32Array(buffer.getMappedRange(), 0, n), h => (h > -1e29 ? h : NaN));
        }));
        for (const resource of [input, output, shares, target, ...read]) resource.destroy();
        return { probe, instance, sun };
    }

    /**
     * The invariant audit of the drawn nodes with the state of the last frame: drawnHeight at samples
     * along their roads and lakes, against what the stamps promise at the level that draws each one,
     * and the stamps of neighbouring nodes on both sides of their shared border.
     */
    async audit(): Promise<Audit> {
        const r = this.resources;
        if (!r || !this.anchor) throw new Error('The audit runs after the first frame.');
        const { device } = r.gpu, nodes = this.nodes;
        const results = await Promise.all(nodes.map(node => this.stamps.samples(node))), lists = results.map(({ samples }) => samples);
        // The water levels of each feature tile that a drawn node reads, once.
        const tiles = new Map(nodes.map((node, i) => [key(featureTile(node) ?? node), results[i]]));
        const sum = (name: 'lakes' | 'mixed' | 'rivers' | 'rising') => [...tiles.values()].reduce((total, result) => total + result[name], 0);
        // A drawn node at the edge of the frame borders culled nodes, whose pages may be absent: only what the frame shows counts.
        const visible = lists.map(list => Array.from({ length: list.length / SAMPLE_WORDS }, (_, k) => this.shows(list, k * SAMPLE_WORDS)));
        const total = visible.reduce((sum, list) => sum + list.filter(Boolean).length, 0), every = Math.max(1, Math.ceil(total / SAMPLES));
        const samples = new Uint32Array(Math.max(1, Math.ceil(total / every)) * SAMPLE_WORDS);
        let n = 0, seen = 0;
        lists.forEach((list, l) => visible[l].forEach((shown, k) => {
            if (shown && seen++ % every === 0) samples.set(list.subarray(k * SAMPLE_WORDS, (k + 1) * SAMPLE_WORDS), SAMPLE_WORDS * n++);
        }));
        // Side by side drawn nodes whose stamp data is known; below level 11 the buffer of the feature tiles is narrower than a texel.
        const drawn = new Map(nodes.map(node => [key(node), node]));
        // The node table of audit.wgsl, then the data of the nodes.
        const table = new Uint32Array((FINEST + 1) * MAP * MAP), origins = this.work!.origins;
        const data: Uint32Array<ArrayBuffer>[] = [table], offsets = new Map<string, number>();
        let words = table.length;
        const offset = (node: Node) => {
            const name = key(node);
            if (!offsets.has(name)) {
                const stamps = this.stamps.get(node);
                offsets.set(name, stamps?.data ? words : -1);
                if (stamps?.data) {
                    data.push(stamps.data);
                    words += stamps.data.length;
                }
            }
            return offsets.get(name)!;
        };
        // The stamp data of the drawn nodes and of their parents, by the LOD map window, for the conflicts near a violation.
        for (const node of [...nodes, ...nodes.map(({ level, x, y }) => ({ level: level - 1, x: x >> 1, y: y >> 1 }))]) {
            const [ox, oy] = origins[node.level] ?? [0, 0], i = node.x - ox, j = node.y - oy, at = offset(node);
            if (i >= 0 && j >= 0 && i < MAP && j < MAP && at >= 0) table[(node.level * MAP + j) * MAP + i] = at + 1;
        }
        const pairs: number[] = [];
        for (const a of nodes) {
            if (a.level < 11 || !featureTile(a)) continue;
            for (const axis of [0, 1]) {
                const b = drawn.get(key({ level: a.level, x: a.x + 1 - axis, y: a.y + axis }));
                if (!b) continue;
                const [da, db] = [offset(a), offset(b)];
                if (da >= 0 || db >= 0) pairs.push(a.x * PAGE, a.y * PAGE, b.x * PAGE, b.y * PAGE, a.level, da < 0 ? 0xffffffff : da, db < 0 ? 0xffffffff : db, axis);
            }
        }
        const packed = new Uint32Array(Math.max(1, words));
        data.reduce((at, list) => (packed.set(list, at), at + list.length), 0);
        const storage = (contents: Uint32Array<ArrayBuffer>) => {
            const buffer = device.createBuffer({ size: Math.max(16, contents.byteLength), usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST | GPUBufferUsage.COPY_SRC });
            device.queue.writeBuffer(buffer, 0, contents);
            return buffer;
        };
        const buffers = [storage(samples.subarray(0, Math.max(1, n) * SAMPLE_WORDS)), storage(packed), storage(new Uint32Array(COUNTS)), storage(new Uint32Array(FOUND * 8)),
            storage(new Uint32Array(pairs.length ? pairs : [0, 0, 0, 0, 0, 0, 0, 0]))];
        const read = [COUNTS * 4, FOUND * 32].map(size => device.createBuffer({ size, usage: GPUBufferUsage.MAP_READ | GPUBufferUsage.COPY_DST }));
        const [audit, apron] = await r.gpu.once(auditPipelines);
        const encoder = device.createCommandEncoder(), pass = encoder.beginComputePass();
        pass.setBindGroup(0, device.createBindGroup({ layout: r.gpu.once(terrainLayouts).audit, entries: [...buffers.slice(0, 4), r.profiles, buffers[4]].map((buffer, binding) => ({ binding, resource: { buffer } })) }));
        pass.setBindGroup(1, r.surfaceGroup);
        pass.setBindGroup(2, r.emptyGroup);
        pass.setBindGroup(3, r.featureGroup);
        if (n) {
            pass.setPipeline(audit);
            pass.dispatchWorkgroups(Math.ceil(n / 64));
        }
        if (pairs.length) {
            pass.setPipeline(apron);
            pass.dispatchWorkgroups(512 / 64, pairs.length / 8);
        }
        pass.end();
        encoder.copyBufferToBuffer(buffers[2], 0, read[0], 0, COUNTS * 4);
        encoder.copyBufferToBuffer(buffers[3], 0, read[1], 0, FOUND * 32);
        device.queue.submit([encoder.finish()]);
        const [counts, found] = await Promise.all(read.map(async buffer => {
            await buffer.mapAsync(GPUMapMode.READ);
            return buffer.getMappedRange().slice(0);
        }));
        for (const buffer of [...buffers, ...read]) buffer.destroy();
        const c = new Uint32Array(counts), f = new Float32Array(found);
        const check = (kind: number) => ({ checked: c[6 * kind], violations: c[6 * kind + 1], conflicts: c[6 * kind + 2], junctions: c[6 * kind + 3], largest: c[6 * kind + 4] / 1e6, junctionLargest: c[6 * kind + 5] / 1e6 });
        return {
            checks: { crossFall: check(1), roadHeight: check(2), inWater: check(3), onBank: check(4), shoreSide: check(5), underDeck: check(6) },
            levels: { lakes: sum('lakes'), mixed: sum('mixed') }, rivers: { checked: sum('rivers'), rising: sum('rising') },
            apron: { checked: c[42], differ: c[43] },
            found: Array.from({ length: Math.min(c[44], FOUND) }, (_, i) => ({
                kind: f[8 * i + 4], level: f[8 * i + 5], s: [f[8 * i], f[8 * i + 1]] as [number, number], drawn: f[8 * i + 2], expected: f[8 * i + 3], detail: f[8 * i + 6],
            })),
        };
    }

    /** Whether the last frame shows an audit sample, at its expected height. */
    private shows(list: Uint32Array, at: number): boolean {
        const seen = this.seen, anchor = this.anchor!;
        if (!seen) return false;
        const [sx, sy] = lodSpace(anchor, list[at] | 0, list[at + 1] | 0), z = new Float32Array(list.buffer, list.byteOffset + 4 * (at + 3), 1)[0];
        const x = cubic(seen.forward, sx, sy, 0), y = cubic(seen.forward, sx, sy, 10), h = z - seen.z - CURVATURE * (x * x + y * y), m = seen.viewProj;
        const clip = [0, 1, 3].map(i => m[i] * x + m[4 + i] * y + m[8 + i] * h + m[12 + i]);
        return clip[2] > 0 && Math.abs(clip[0]) < 0.98 * clip[2] && Math.abs(clip[1]) < 0.98 * clip[2];
    }

    private rule(eye: { anchor: Anchor; above: number }, pixels: number): Rule {
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

    private writeStamps(r: Resources, stamps: Stamp[]): void {
        const { jobs, data } = stampJobs(stamps);
        r.gpu.device.queue.writeBuffer(r.stampJobs, 0, jobs);
        if (data.length) r.gpu.device.queue.writeBuffer(r.stampData, 0, data);
    }

    /** Writes the frame's buffers; true when the drawn nodes differ from the last frame's. */
    private write(r: Resources, anchor: Anchor, rule: Rule, forward: Float64Array, work: Work, above: number): boolean {
        const { device } = r.gpu;
        const surface = new ArrayBuffer(SURFACE_BYTES), ints = new Int32Array(surface), floats = new Float32Array(surface);
        ints.set(anchor.whole, 0);
        floats.set([...anchor.part, anchor.unit, above, rule.range], 2);
        new Uint32Array(surface)[7] = rule.finest;
        floats.set(forward, 8);
        work.origins.forEach((origin, level) => ints.set(origin, 28 + 4 * level));
        device.queue.writeBuffer(r.surface, 0, surface);
        // The points of the probe in LOD space, by the linear part of the cubic at the eye.
        const [a, b, c, d] = [forward[1], forward[2], forward[11], forward[12]], k = PROBE_STEP / (a * d - b * c);
        device.queue.writeBuffer(r.points, 0, new Float32Array([0, 0, d * k, -c * k, -d * k, c * k, -b * k, a * k, b * k, -a * k]));
        if (work.map) device.queue.writeBuffer(r.map, 0, work.map);
        if (work.masks) device.queue.writeBuffer(r.masks, 0, work.masks);
        for (const { page, data } of work.uploads) {
            device.queue.writeTexture({ texture: r.heights, origin: origin(page.slot) }, data.texels, { bytesPerRow: PAGE * data.texels.BYTES_PER_ELEMENT }, [PAGE, PAGE, 1]);
            device.queue.writeBuffer(r.scales, 8 * page.slot, new Float32Array([data.offset, data.scale]));
        }
        const heightsOf = new Set(work.stamps.filter(({ mode }) => mode & MODE.HEIGHTS).map(({ page }) => page));
        device.queue.writeBuffer(r.synthJobs, 0, jobs([
            ...work.syntheses.map(({ page, stored }) => ({ page, stored, slot: page.slot, stamped: heightsOf.has(page) })),
            ...copies(work).map(({ page, target }) => ({ page, stored: 0, slot: target, stamped: true })),
        ]));
        if (work.stamps.length) this.writeStamps(r, work.stamps);
        device.queue.writeBuffer(r.extent, 0, new Uint32Array(2 * ARRIVALS).map((_, i) => (i % 2 ? 0 : 0xffffffff)));
        const instances = new Int32Array(4 * this.nodes.length);
        this.nodes.forEach(({ level, x, y }, i) => instances.set([x * size(level) - anchor.whole[0], y * size(level) - anchor.whole[1], level, 0], 4 * i));
        device.queue.writeBuffer(r.nodes, 0, instances);
        device.queue.writeBuffer(r.holes, 0, holes(this.boxes, anchor));
        const changed = instances.length !== this.instances.length || instances.some((v, i) => v !== this.instances[i]);
        this.instances = instances;
        this.water.write();
        this.structures.write();
        return changed;
    }
}

/** The terrain's own bind group layouts; groups 1 to 3 are those of gpu/layouts.ts. */
function terrainLayouts({ device }: Gpu) {
    const layout = (entries: [GPUShaderStageFlags, Omit<GPUBindGroupLayoutEntry, 'binding' | 'visibility'>][]) =>
        device.createBindGroupLayout({ entries: entries.map(([visibility, entry], binding) => ({ binding, visibility, ...entry })) });
    const read: GPUBufferBindingLayout = { type: 'read-only-storage' }, write: GPUBufferBindingLayout = { type: 'storage' }, C = GPUShaderStage.COMPUTE;
    return {
        mesh: layout([[GPUShaderStage.VERTEX | GPUShaderStage.FRAGMENT, { buffer: { type: 'uniform' } }], [GPUShaderStage.VERTEX, { buffer: read }], [GPUShaderStage.VERTEX, { buffer: read }]]),
        job: layout([[C, { buffer: read }], [C, { buffer: write }], [C, { buffer: write }]]),
        quantize: layout([[C, { buffer: read }], [C, { buffer: read }], [C, { buffer: read }], [C, { buffer: write }], [C, { buffer: write }]]),
        probe: layout([[C, { buffer: read }], [C, { buffer: write }]]),
        check: layout([[GPUShaderStage.VERTEX, { buffer: read }]]),
        stamp: layout([
            [C, { buffer: read }], [C, { buffer: read }], [C, { buffer: write }], [C, { buffer: write }], [C, { buffer: { type: 'uniform' } }],
            [C, { storageTexture: { format: 'rgba8unorm', viewDimension: '2d-array' } }],
        ]),
        snap: device.createBindGroupLayout({
            entries: [
                { binding: 0, visibility: C, buffer: read }, { binding: 6, visibility: C, texture: { sampleType: 'float', viewDimension: '2d-array' } },
                { binding: 7, visibility: C, storageTexture: { format: 'r32uint', viewDimension: '2d-array' } },
            ],
        }),
        // The audit, the probe and the check read the feature mask in group 3, where the terrain draw has it after the ground.
        empty: layout([]),
        audit: layout([[C, { buffer: read }], [C, { buffer: read }], [C, { buffer: write }], [C, { buffer: write }], [C, { buffer: { type: 'uniform' } }], [C, { buffer: read }]]),
    };
}

/** A terrain shader, with the includes of `clip-distances` on a device that has it. */
const shader = ({ device }: Gpu, file: string) => device.createShaderModule({ label: file, code: wgsl(`terrain/${file}.wgsl`, device.features.has('clip-distances') ? CLIP_DISTANCES : {}) });

/** A compute pipeline of a terrain shader, whose groups take the layouts `groups`. */
function compute(gpu: Gpu, file: string, entryPoint: string, groups: GPUBindGroupLayout[]): Promise<GPUComputePipeline> {
    return gpu.device.createComputePipelineAsync({ label: entryPoint, layout: gpu.device.createPipelineLayout({ bindGroupLayouts: groups }), compute: { module: shader(gpu, file), entryPoint } });
}

/** The draw of the nodes: the near nodes with the whole look (NEAR of look.wgsl), the far ones without, or with the holes of the tunnels' portals. */
function meshPipeline(gpu: Gpu, near: boolean, holes = false): Promise<GPURenderPipeline> {
    const { device } = gpu, L = gpu.once(terrainLayouts), shared = gpu.once(layouts), module = shader(gpu, 'terrain');
    return device.createRenderPipelineAsync({
        label: holes ? 'terrain with holes' : near ? 'terrain' : 'far terrain',
        layout: device.createPipelineLayout({ bindGroupLayouts: [L.mesh, shared.surface, shared.ground, shared.features] }),
        vertex: { module, entryPoint: 'vertex_main', constants: { HOLES: holes ? 1 : 0 } },
        fragment: { module, entryPoint: 'fragment_main', targets: [{ format: sceneFormat(gpu) }], constants: { NEAR: near ? 1 : 0 } },
        // The far nodes cull back faces: no snap moves their vertices, so no triangle turns over, and a
        // ray from an eye above them meets a front face first. The near nodes draw both faces:
        // the eye can be under the near mesh for some frames, and its underside then hides what is behind.
        primitive: { topology: 'triangle-list', cullMode: near ? 'none' : 'back' },
        depthStencil: { format: DEPTH, depthWriteEnabled: true, depthCompare: 'greater' },
    });
}

type Pipelines = Awaited<ReturnType<typeof terrainPipelines>>;

/** What the terrain draws and computes with from its first frame. */
export async function terrainPipelines(gpu: Gpu) {
    const L = gpu.once(terrainLayouts), { surface, features } = gpu.once(layouts);
    const [mesh, farMesh, synthesize, quantize, probe, stamp, snap] = await Promise.all([
        meshPipeline(gpu, true), meshPipeline(gpu, false),
        compute(gpu, 'pages', 'synthesize', [L.job, surface]),
        compute(gpu, 'quantize', 'quantize', [L.quantize]),
        compute(gpu, 'probe', 'probe', [L.probe, surface, L.empty, features]),
        compute(gpu, 'stamp', 'stamp', [L.stamp, surface]),
        compute(gpu, 'stamp', 'snap', [L.snap]),
    ]);
    return { mesh, farMesh, synthesize, quantize, probe, stamp, snap };
}

/** The draw of the nodes with a tunnel's portal, with its hole. */
const holedPipeline = (gpu: Gpu) => meshPipeline(gpu, true, true);

/** The check's draw and the probe of the share of the sun: only tests check. */
function checkPipelines(gpu: Gpu): Promise<[GPURenderPipeline, GPUComputePipeline]> {
    const { device } = gpu, L = gpu.once(terrainLayouts), { surface, features, ground } = gpu.once(layouts), module = shader(gpu, 'probe');
    return Promise.all([
        device.createRenderPipelineAsync({
            label: 'check', layout: device.createPipelineLayout({ bindGroupLayouts: [L.check, surface, L.empty, features] }),
            vertex: { module, entryPoint: 'check_vertex' },
            fragment: { module, entryPoint: 'check_fragment', targets: [{ format: 'r32float' }] },
            primitive: { topology: 'point-list' },
        }),
        compute(gpu, 'sun-probe', 'sun_probe', [L.probe, surface, ground, features]),
    ]);
}

/** The audit's pipelines: SwiftShader takes half a minute for them, and only tests audit. */
function auditPipelines(gpu: Gpu): Promise<GPUComputePipeline[]> {
    const L = gpu.once(terrainLayouts), { surface, features } = gpu.once(layouts);
    return Promise.all(['audit', 'apron'].map(entry => compute(gpu, 'audit', entry, [L.audit, surface, L.empty, features])));
}

/** The portals' boxes as `holes` of terrain.wgsl reads them, about the anchor; each box once, MAX_HOLES at most. */
function holes(lists: Float64Array[], anchor: Anchor): Float32Array<ArrayBuffer> {
    const out = new Float32Array(4 * (1 + 3 * MAX_HOLES)), seen = new Set<string>();
    let n = 0;
    for (const list of lists) for (let i = 0; i < list.length && n < MAX_HOLES; i += BOX) {
        const name = `${list[i]},${list[i + 1]}`;
        if (seen.has(name)) continue;
        seen.add(name);
        out.set([list[i] - anchor.whole[0] - anchor.part[0], list[i + 1] - anchor.whole[1] - anchor.part[1], ...list.subarray(i + 2, i + BOX)], 4 + 12 * n++);
    }
    out[0] = n;
    return out;
}

/** Where a page lies: one of 8 × 8 tiles of a layer, as `in_layer` in drawn-height.wgsl has it. */
const origin = (slot: number): [number, number, number] => [PAGE * (slot & 7), PAGE * ((slot >> 3) & 7), slot >> 6];

/** Jobs as `Job` in the WGSL: the node's first texel, its level, the stored level, the layer and whether a stamp follows. */
function jobs(list: { page: Page; stored: number; slot: number; stamped: boolean }[]): ArrayBuffer {
    const data = new ArrayBuffer(Math.max(1, list.length) * JOB_BYTES), ints = new Int32Array(data);
    list.forEach(({ page: { node: { level, x, y } }, stored, slot, stamped }, k) => ints.set([x * PAGE, y * PAGE, level, stored, slot, stamped ? 1 : 0], (JOB_BYTES / 4) * k));
    return data;
}

/** Stamp jobs as `StampJob` in stamp.wgsl, with their nodes' data packed after each other from `base`. */
function stampJobs(stamps: Stamp[], base = 0): { jobs: ArrayBuffer; data: Uint32Array<ArrayBuffer> } {
    const jobs = new ArrayBuffer(Math.max(1, stamps.length) * STAMP_JOB_BYTES), words = new Uint32Array(jobs), ints = new Int32Array(jobs);
    const data = new Uint32Array(stamps.reduce((sum, { data }) => sum + (data?.length ?? 0), 0));
    let at = 0;
    stamps.forEach(({ page, out, data: node, mode }, k) => {
        const w = (STAMP_JOB_BYTES / 4) * k;
        ints.set([page.node.x * PAGE, page.node.y * PAGE], w);
        words.set([page.node.level, node ? base + at : 0xffffffff, out, !(mode & MODE.HEIGHTS) ? page.drawn! : page.kind === 'stored' ? page.slot : SYNTHESIZED, mode, page.mask ?? 0], w + 2);
        if (node) {
            data.set(node, at);
            at += node.length;
        }
    });
    return { jobs, data };
}

/**
 * The triangles of the grid mesh, with vertex j × 65 + i at cell column i and row j, counter-clockwise
 * from above. They go row by row in strips of STRIP columns: the vertex cache then still holds the
 * strip's last row of vertices, and most vertices run once.
 */
export function gridIndices(): Uint16Array<ArrayBuffer> {
    const out = new Uint16Array(6 * GRID * GRID), n = GRID + 1;
    let k = 0;
    for (let strip = 0; strip < GRID; strip += STRIP) for (let j = 0; j < GRID; j++) for (let i = strip; i < strip + STRIP; i++) {
        const a = j * n + i;
        out.set([a, a + n, a + 1, a + 1, a + n, a + n + 1], k);
        k += 6;
    }
    return out;
}
