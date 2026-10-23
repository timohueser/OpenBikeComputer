import type { Gpu } from '../gpu/device';
import { wgsl } from '../gpu/wgsl';
import { CURVATURE, type Frame } from '../frame/frame';
import { MESHES, TEMPLATES } from '../models/house';
import type { HouseStyle, Rules } from '../rules/regions';
import type { FeatureTiles } from '../terrain/stamps';
import { OUTLINE_WORDS, RECORD_WORDS, type TileHouses } from './house-records';

/** Houses on the GPU at most, and the bytes of outline vertices. */
const HOUSES = 2 ** 16, OUTLINE_BYTES = 2 * 2 ** 20;
/** The houses of the zoom 14 tiles within this many metres of the eye draw; tiles kept, and tile requests in flight at most. */
const RADIUS_M = 8000, KEPT = 400, REQUESTS = 6;
const ZOOM = 14, EARTH_M = 40075016.686;
/** Words of `Kept`, bytes of `Drawn` and words of `Style` in houses.wgsl; the bytes of `Params` and where its styles start; the stride of the dynamic offset of `DrawOf`. */
const FIT_WORDS = 4, DRAWN_BYTES = 160, STYLE_WORDS = 32, STYLES_AT = 48 + 64 * 16 + 256 * 16, STYLES = 32, PARAMS_BYTES = STYLES_AT + STYLES * STYLE_WORDS * 4, DRAW_STRIDE = 256;
/** Houses drawn in a frame at most; past ROOM of them, the narrowest on screen go first. */
const DRAWN = 2 ** 14, ROOM = [0.7, 0.9];
/** Tiles that one fit pass reaches at most, as `ranges` in houses.wgsl. */
const RANGES = 256;
/**
 * A tile is in view while a box over it, grown by GROW_M for the houses that reach past it, from
 * LOWEST_M to HIGHEST_M above sea level, meets the frustum.
 */
const GROW_M = 150, LOWEST_M = -500, HIGHEST_M = 4900;
/** Two draws per template: in full, and simple. */
const DRAWS = 2 * TEMPLATES.length;
/**
 * The houses fit their ground again in every TURNS-th frame that moves, and in the first that does
 * not: any refit makes the frame wait for the longest chain of ground samples, however few refit.
 */
const TURNS = 4;
/** Houses per workgroup of `refit`: nine lanes each, and of `fit`: one each, as in houses.wgsl. */
const PER_GROUP = 7, FIT_GROUP = 64;
/** Words of the draws and the drawn count before the lists, as LISTS in houses.wgsl. */
const LISTS = 36;
const TIMBER = ['none', 'upper', 'all', 'frame'];
/**
 * The pad of a house, as `Params` in houses.wgsl: the floor stands `plinth` metres over the lowest
 * ground about the footprint, and at most `burial` storeys under the highest, a full storey where
 * the ground falls by `steep` storeys or more; the walls reach `margin` metres under the lowest. A
 * house wider than `full` pixels draws in full detail; one narrower than `least` not at all.
 */
export const PAD = { burial: 0.5, steep: [2, 3], plinth: 0.3, margin: 1.0, full: 40, least: 1.0 };

/** What the fit culls the tiles with: the camera-relative view and projection, and the eye in the frame. */
export interface HouseCamera { viewProj: Float32Array; eye: [number, number, number] }

/** Houses on the GPU, their tiles, and the houses that a recent frame drew in full and simple. */
export interface HouseStats { houses: number; tiles: number; full: number; simple: number }

interface Resources {
    device: GPUDevice;
    fit: GPUComputePipeline;
    refit: GPUComputePipeline;
    templates: GPURenderPipeline;
    outlines: GPURenderPipeline;
    /** Per fits buffer: the groups of the compute; the group of the draws. */
    computeGroups: GPUBindGroup[];
    renderGroup: GPUBindGroup;
    houses: GPUBuffer;
    fits: GPUBuffer[];
    draws: GPUBuffer;
    readbacks: GPUBuffer[];
    params: GPUBuffer;
    outlineVertices: GPUBuffer;
    surface: GPUBindGroup;
    features: GPUBindGroup;
}

/** The style table of houses.wgsl: `Style` per style, then the styles of each region. */
function styleWords(rules: Rules): { styles: Uint32Array<ArrayBuffer>; regions: Uint32Array<ArrayBuffer> } {
    const list: HouseStyle[] = rules.styles.length ? rules.styles : [FALLBACK];
    const words = new ArrayBuffer(list.length * STYLE_WORDS * 4), u = new Uint32Array(words), f = new Float32Array(words);
    const colour = (hex: string) => parseInt(hex.slice(1, 3), 16) | (parseInt(hex.slice(3, 5), 16) << 8) | (parseInt(hex.slice(5, 7), 16) << 16);
    list.forEach((style, i) => {
        const at = i * STYLE_WORDS, { palette } = style, lists = [palette.render, palette.wood, palette.roof, palette.door, palette.shutter];
        f.set([style.pitch[0], style.pitch[1], style.eave, style.verge, style.hip], at);
        u.set([Math.max(0, TIMBER.indexOf(style.timber)), +style.shutters | (+!!style.balconies << 1) | (+(style.spire === 'onion') << 2), style.chimneys], at + 5);
        f[at + 8] = style.storeys ?? 0;
        u.set([colour(palette.plinth), colour(palette.chimney), lists.reduce((n, c, k) => n | (Math.min(c.length, 4) << (4 * k)), 0)], at + 9);
        lists.forEach((c, k) => c.slice(0, 4).forEach((hex, j) => (u[at + 12 + 4 * k + j] = colour(hex))));
    });
    return { styles: u, regions: Uint32Array.from(rules.regions.flatMap(({ houses }) => houses)) };
}

/** The style of a release without a ruleset: the prototype's `house`. */
const FALLBACK: HouseStyle = {
    name: 'House', pitch: [30, 40], eave: 0.5, verge: 0.4, hip: 0, timber: 'none', shutters: false, chimneys: 1,
    palette: { render: ['#f1ece0'], wood: ['#5e4030'], roof: ['#6a5d52'], door: ['#5e4030'], shutter: ['#3f6a42'], plinth: '#8f8a80', chimney: '#a39a8e' },
};

/**
 * The houses of the feature tiles about the eye. A worker makes the records of each zoom 14 tile;
 * the records of the drawn tiles go to the GPU together, nearest tile first. Each frame houses.wgsl
 * fits every house to the drawn surface and picks its draw.
 */
export class Houses {
    private resources?: Resources;
    private readonly worker?: Worker;
    private readonly tiles = new Map<string, TileHouses | null>();
    private readonly pending = new Set<string>();
    private readonly waiting = new Map<number, (result: TileHouses | null) => void>();
    private next = 0;
    private drawn = '';
    private drawnTiles = new Set<string>();
    private count = 0;
    private outlineVertices = 0;
    private regions = new Uint32Array(0);
    private styles = new Uint32Array(0);
    /** The tiles on the GPU: their address and their records. */
    private ranges: { x: number; y: number; start: number; count: number }[] = [];
    private camera?: HouseCamera;
    /** Whether the records moved since the last encode: their fits move with them there. */
    private moved = false;
    private drawnCounts = { full: 0, simple: 0 };
    /** Frames that moved, and whether one moved since the last refit. */
    private frames = 0;
    private stale = false;
    /** The fits buffer that holds the fits of the houses on the GPU. */
    private set = 0;
    /** Moves of the fits for the next encode, when the records moved: from the old buffer to the new, in houses; a move from -1 clears. */
    private moves: { from: number; to: number; count: number }[] = [];
    /** The narrowest house that draws, in pixels: PAD.least while the pool of drawn houses has room. */
    private least = PAD.least;
    /** Whether the last update put other houses on the GPU. */
    newContent = false;

    constructor(private readonly frame: Frame, private readonly features: FeatureTiles | undefined, private readonly rules: Rules, private readonly changed: () => void) {
        if (!features) return;
        this.worker = new Worker(new URL('./house-worker.ts', import.meta.url), { type: 'module' });
        this.worker.onmessage = ({ data }) => {
            const resolve = this.waiting.get(data.id);
            this.waiting.delete(data.id);
            if (data.error) console.warn(`House tile: ${data.error}`);
            resolve?.(data.error ? null : data.result);
        };
    }

    get idle(): boolean {
        return this.pending.size === 0 && !this.working;
    }

    /** True while the houses need a frame to fit their ground. */
    get working(): boolean {
        return this.stale;
    }

    get stats(): HouseStats {
        return { houses: this.count, tiles: [...this.tiles.values()].filter(Boolean).length, ...this.drawnCounts };
    }

    async attach(gpu: Gpu, frame: GPUBuffer, surface: { layout: GPUBindGroupLayout; group: GPUBindGroup }, features: { layout: GPUBindGroupLayout; group: GPUBindGroup },
        ground: { render: GPUBindGroupLayout; compute: GPUBindGroupLayout }, format: GPUTextureFormat, depthFormat: GPUTextureFormat): Promise<void> {
        this.resources = undefined;
        this.drawn = '';
        const { device } = gpu, C = GPUShaderStage.COMPUTE, VF = GPUShaderStage.VERTEX | GPUShaderStage.FRAGMENT;
        const storage = GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST;
        const { styles, regions } = styleWords(this.rules);
        this.regions = regions;
        this.styles = styles.subarray(0, STYLES * STYLE_WORDS);
        const words = new Uint32Array(TEMPLATES.flatMap(name => [...MESHES[name].words]));
        const upload = (pool: string, data: Uint32Array<ArrayBuffer>) => {
            const buffer = gpu.buffer(pool, Math.max(16, data.byteLength), storage);
            device.queue.writeBuffer(buffer, 0, data);
            return buffer;
        };
        const b = {
            houses: gpu.buffer('houses', HOUSES * RECORD_WORDS * 4, storage),
            fits: [0, 1].map(() => gpu.buffer('houses', HOUSES * FIT_WORDS * 4, storage | GPUBufferUsage.COPY_SRC)),
            drawn: gpu.buffer('houses', DRAWN * DRAWN_BYTES, GPUBufferUsage.STORAGE),
            draws: gpu.buffer('houses', (LISTS + DRAWS * DRAWN + HOUSES) * 4, GPUBufferUsage.STORAGE | GPUBufferUsage.INDIRECT | GPUBufferUsage.COPY_DST | GPUBufferUsage.COPY_SRC),
            drawOf: gpu.buffer('houses', DRAWS * DRAW_STRIDE, GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST),
            params: gpu.buffer('houses', PARAMS_BYTES, GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST),
            templates: upload('houses', words),
            outlineVertices: gpu.buffer('houses', OUTLINE_BYTES, GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST),
        };
        const drawOf = new Uint32Array(DRAWS * DRAW_STRIDE / 4);
        for (let d = 0; d < DRAWS; d++) drawOf.set([d * DRAWN, DRAWN], d * DRAW_STRIDE / 4);
        device.queue.writeBuffer(b.drawOf, 0, drawOf);
        const entry = (binding: number, visibility: number, type: GPUBufferBindingType, hasDynamicOffset = false): GPUBindGroupLayoutEntry => ({ binding, visibility, buffer: { type, hasDynamicOffset } });
        const computeLayout = device.createBindGroupLayout({ entries: [
            entry(0, C, 'uniform'), entry(1, C, 'read-only-storage'), entry(2, C, 'storage'), entry(5, C, 'storage'), entry(11, C, 'uniform'), entry(12, C, 'storage'),
        ] });
        const renderLayout = device.createBindGroupLayout({ entries: [
            entry(0, VF, 'uniform'), entry(6, VF, 'read-only-storage'),
            { binding: 7, visibility: VF, buffer: { type: 'uniform', hasDynamicOffset: true, minBindingSize: 8 } }, entry(8, VF, 'read-only-storage'), entry(13, VF, 'read-only-storage'),
        ] });
        const module = device.createShaderModule({ label: 'houses', code: wgsl('structures/houses.wgsl') });
        const renderPipelineLayout = device.createPipelineLayout({ bindGroupLayouts: [renderLayout, surface.layout, ground.render] });
        const render = (entryPoint: string, buffers: GPUVertexBufferLayout[]) => device.createRenderPipelineAsync({
            label: 'houses', layout: renderPipelineLayout, vertex: { module, entryPoint, buffers },
            fragment: { module, entryPoint: 'house_fragment', targets: [{ format }] },
            primitive: { topology: 'triangle-list', cullMode: 'back' },
            depthStencil: { format: depthFormat, depthWriteEnabled: true, depthCompare: 'greater' },
        });
        const fitLayout = device.createPipelineLayout({ bindGroupLayouts: [computeLayout, surface.layout, ground.compute, features.layout] });
        const compute = (entryPoint: string) => device.createComputePipelineAsync({ label: 'houses', layout: fitLayout, compute: { module, entryPoint } });
        const [fit, refit, templates, outlines] = await Promise.all([
            compute('fit'), compute('refit'),
            render('template_vertex', []),
            render('outline_vertex', [{ arrayStride: OUTLINE_WORDS * 4, attributes: [
                { shaderLocation: 0, offset: 0, format: 'sint32x2' }, { shaderLocation: 1, offset: 8, format: 'uint32' }, { shaderLocation: 2, offset: 12, format: 'float32' },
            ] }]),
        ]);
        const bind = (layout: GPUBindGroupLayout, list: [number, GPUBuffer, number?][]) => device.createBindGroup({ layout, entries: list.map(([binding, buffer, size]) => ({ binding, resource: { buffer, size } })) });
        this.resources = {
            device, fit, refit, templates, outlines, houses: b.houses, fits: b.fits, draws: b.draws, params: b.params,
            readbacks: [0, 1].map(() => gpu.buffer('houses', LISTS * 4, GPUBufferUsage.MAP_READ | GPUBufferUsage.COPY_DST)), outlineVertices: b.outlineVertices,
            surface: surface.group, features: features.group,
            computeGroups: [0, 1].map(k => bind(computeLayout, [[0, frame], [1, b.houses], [2, b.fits[k]], [5, b.draws], [11, b.params], [12, b.drawn]])),
            renderGroup: bind(renderLayout, [[0, frame], [6, b.draws], [7, b.drawOf, 8], [8, b.templates], [13, b.drawn]]),
        };
    }

    /** Asks for the tiles about the eye at (lon, lat) and puts the houses of the known ones on the GPU, nearest first. */
    update(lonLat: [number, number], camera: HouseCamera | undefined): void {
        this.newContent = false;
        this.camera = camera;
        const r = this.resources;
        if (!r || !this.features) return;
        const n = 2 ** ZOOM, ex = (lonLat[0] + 180) / 360 * n, lat = lonLat[1] * Math.PI / 180;
        const ey = (1 - Math.log(Math.tan(lat) + 1 / Math.cos(lat)) / Math.PI) / 2 * n;
        const reach = RADIUS_M / (EARTH_M * Math.cos(lat) / n), wanted: { name: string; x: number; y: number; d: number }[] = [];
        for (let y = Math.floor(ey - reach); y <= Math.floor(ey + reach); y++) for (let x = Math.floor(ex - reach); x <= Math.floor(ex + reach); x++) {
            const d = Math.hypot(Math.max(x - ex, 0, ex - x - 1), Math.max(y - ey, 0, ey - y - 1));
            if (d <= reach) wanted.push({ name: `${x}/${y}`, x, y, d });
        }
        wanted.sort((a, b) => a.d - b.d);
        for (const { name, x, y } of wanted) {
            if (this.tiles.has(name) || this.pending.has(name) || this.pending.size >= REQUESTS) continue;
            this.pending.add(name);
            this.load(x, y).then(result => this.keep(name, result), error => {
                console.warn(`House tile ${name}: ${error instanceof Error ? error.message : error}`);
                this.keep(name, null);
            }).finally(() => {
                this.pending.delete(name);
                this.changed();
            });
        }
        const known = wanted.filter(({ name }) => this.tiles.get(name)).map(({ name }) => name), drawn = known.join(' ');
        if (drawn === this.drawn) return;
        this.drawn = drawn;
        this.drawnTiles = new Set(known);
        this.newContent = true;
        // The records of the tiles, and their outline vertices with the index of their house among all records.
        // A tile whose records do not fit draws none; one whose outlines do not fit draws its other houses.
        let houses = 0, vertices = 0;
        const fitting: { name: string; tile: TileHouses }[] = [];
        for (const name of known) {
            const tile = this.tiles.get(name)!, n = tile.records.length / RECORD_WORDS;
            if (houses + n > HOUSES) continue;
            fitting.push({ name, tile });
            houses += n;
        }
        const outlined = fitting.map(({ tile: { outlines } }) => {
            const fits = (vertices + outlines.length / OUTLINE_WORDS) * OUTLINE_WORDS * 4 <= OUTLINE_BYTES;
            if (fits) vertices += outlines.length / OUTLINE_WORDS;
            return fits;
        });
        const records = new Uint32Array(houses * RECORD_WORDS), outlines = new Uint32Array(vertices * OUTLINE_WORDS);
        let base = 0, at = 0;
        // A tile that stays keeps the fits of its houses: they move with its records.
        // Two uploads before an encode: the fits are still where the first found them, so none stay.
        const before = new Map(this.moved ? [] : this.ranges.map(range => [`${range.x}/${range.y}`, range]));
        this.moves = [];
        this.ranges = [];
        fitting.forEach(({ name, tile }, k) => {
            const [x, y] = name.split('/').map(Number), count = tile.records.length / RECORD_WORDS;
            this.moves.push({ from: before.get(name)?.count === count ? before.get(name)!.start : -1, to: base, count });
            this.ranges.push({ x, y, start: base, count });
            records.set(tile.records, base * RECORD_WORDS);
            for (let i = 0; outlined[k] && i < tile.outlines.length; i += OUTLINE_WORDS, at += OUTLINE_WORDS) {
                outlines.set(tile.outlines.subarray(i, i + OUTLINE_WORDS), at);
                outlines[at + 2] += base;
            }
            base += tile.records.length / RECORD_WORDS;
        });
        if (records.length) r.device.queue.writeBuffer(r.houses, 0, records);
        if (outlines.length) r.device.queue.writeBuffer(r.outlineVertices, 0, outlines);
        this.count = houses;
        this.outlineVertices = vertices;
        this.moved = true;
    }

    /**
     * Encodes the fit of the houses of the tiles in view, which fills the draws, and in some frames
     * their refit; a frame that moved nothing (`moving` false) keeps the draws of the frame before.
     * Call the returned function after the submit.
     */
    encode(encoder: GPUCommandEncoder, ground: GPUBindGroup | undefined, moving: boolean, timing: () => GPUComputePassTimestampWrites | undefined = () => undefined): () => void {
        const r = this.resources;
        if (!moving && !this.stale && !this.moved) return () => {};
        const refitting = moving ? ++this.frames % TURNS === 0 : this.stale || this.moved;
        this.stale = !refitting;
        if (!r || !ground || !this.count) return () => {};
        const args = new Uint32Array(LISTS);
        let first = 0;
        TEMPLATES.forEach((name, t) => {
            const { full, simple } = MESHES[name];
            args.set([full, 0, first, 0, simple, 0, first, 0], 8 * t);
            first += full;
        });
        r.device.queue.writeBuffer(r.draws, 0, args);
        const shown = this.ranges.filter(range => this.inView(range)).slice(0, RANGES);
        const params = new ArrayBuffer(PARAMS_BYTES);
        new Uint32Array(params).set([this.count, DRAWN]);
        new Float32Array(params).set([PAD.burial, PAD.plinth, PAD.margin, PAD.full, this.least, ...PAD.steep], 2);
        new Uint32Array(params)[10] = shown.length;
        new Uint32Array(params, 48).set(this.regions.subarray(0, 256));
        // Each tile's first workgroup of `fit` and of `refit`, so that they run no workgroup without houses.
        const ranges = new Uint32Array(params, 48 + 64 * 16, 4 * RANGES);
        let fits = 0, refits = 0;
        shown.forEach(({ start, count }, k) => {
            ranges.set([start, count, fits, refits], 4 * k);
            fits += Math.ceil(count / FIT_GROUP);
            refits += Math.ceil(count / PER_GROUP);
        });
        new Uint32Array(params, STYLES_AT).set(this.styles);
        r.device.queue.writeBuffer(r.params, 0, params);
        if (this.moved) {
            const from = r.fits[this.set], to = r.fits[this.set ^ 1], bytes = FIT_WORDS * 4;
            for (const move of this.moves) {
                if (move.from < 0) encoder.clearBuffer(to, move.to * bytes, move.count * bytes);
                else encoder.copyBufferToBuffer(from, move.from * bytes, to, move.to * bytes, move.count * bytes);
            }
            this.set ^= 1;
        }
        this.moved = false;
        const pass = encoder.beginComputePass({ label: 'houses', timestampWrites: timing() });
        pass.setBindGroup(0, r.computeGroups[this.set]);
        pass.setBindGroup(1, r.surface);
        pass.setBindGroup(2, ground);
        pass.setBindGroup(3, r.features);
        if (refitting && refits) {
            pass.setPipeline(r.refit);
            pass.dispatchWorkgroups(refits);
        }
        pass.setPipeline(r.fit);
        if (fits) pass.dispatchWorkgroups(fits);
        pass.end();
        // The counts of the draws, for the HUD and the room in the pool.
        const readback = r.readbacks.pop();
        if (!readback) return () => {};
        encoder.copyBufferToBuffer(r.draws, 0, readback, 0, LISTS * 4);
        return () => readback.mapAsync(GPUMapMode.READ).then(() => {
            const counts = new Uint32Array(readback.getMappedRange());
            const sum = (lod: number) => TEMPLATES.reduce((n, _, t) => n + counts[4 * (2 * t + lod) + 1], 0);
            this.drawnCounts = { full: sum(0), simple: sum(1) };
            // A full pool draws the houses narrower on screen no more, until it has room again.
            const used = counts[LISTS - 4] / DRAWN;
            this.least = used > ROOM[1] ? this.least * 1.25 : used < ROOM[0] ? Math.max(PAD.least, this.least / 1.1) : this.least;
            readback.unmap();
            r.readbacks.push(readback);
        }, () => {});
    }

    draw(pass: GPURenderPassEncoder, ground: GPUBindGroup): void {
        const r = this.resources;
        if (!r || !this.count) return;
        pass.setPipeline(r.templates);
        pass.setBindGroup(1, r.surface);
        pass.setBindGroup(2, ground);
        for (let d = 0; d < DRAWS; d++) {
            pass.setBindGroup(0, r.renderGroup, [d * DRAW_STRIDE]);
            pass.drawIndirect(r.draws, d * 16);
        }
        if (!this.outlineVertices) return;
        pass.setPipeline(r.outlines);
        pass.setBindGroup(0, r.renderGroup, [0]);
        pass.setVertexBuffer(0, r.outlineVertices);
        pass.draw(this.outlineVertices);
    }

    /** Whether a box over a tile meets the frustum of the camera, as `visible` in houses.wgsl tests a house. */
    private inView({ x, y }: { x: number; y: number }): boolean {
        if (!this.camera) return true;
        const { viewProj: m, eye } = this.camera, n = 2 ** ZOOM;
        const lonLat = (tx: number, ty: number): [number, number] => [tx / n * 360 - 180, Math.atan(Math.sinh(Math.PI * (1 - 2 * ty / n))) * 180 / Math.PI];
        const corners: number[][] = [];
        for (const [tx, ty] of [[x, y], [x + 1, y], [x + 1, y + 1], [x, y + 1]]) {
            const [east, north] = this.frame.local(...lonLat(tx, ty)), dx = east - eye[0], dy = north - eye[1], drop = CURVATURE * (dx * dx + dy * dy);
            corners.push([dx, dy, LOWEST_M - eye[2] - drop], [dx, dy, HIGHEST_M - eye[2] - drop]);
        }
        // The left, right, bottom and top planes, as rows w ± x and w ± y of the matrix.
        return [[0, 1], [0, -1], [1, 1], [1, -1]].every(([row, sign]) => {
            const a = m[3] + sign * m[row], b = m[7] + sign * m[4 + row], c = m[11] + sign * m[8 + row], d = m[15] + sign * m[12 + row];
            return corners.some(([px, py, pz]) => a * px + b * py + c * pz + d >= -GROW_M * Math.hypot(a, b, c));
        });
    }

    private async load(x: number, y: number): Promise<TileHouses | null> {
        const body = await this.features!(ZOOM, x, y);
        if (!body) return null;
        const id = this.next++;
        return new Promise(resolve => {
            this.waiting.set(id, resolve);
            this.worker!.postMessage({ id, tile: { level: ZOOM, x, y }, body }, [body]);
        });
    }

    private keep(name: string, result: TileHouses | null): void {
        this.tiles.set(name, result);
        for (const [old] of this.tiles) {
            if (this.tiles.size <= KEPT) break;
            if (!this.drawnTiles.has(old)) this.tiles.delete(old);
        }
    }
}
