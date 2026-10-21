import { worldPixel } from '../../planner/layers/mercator';
import { seasonDay } from '../../planner/layers/snow';
import { EARTH_RADIUS, tileLonLat } from '../frame/frame';
import { UNITS } from '../frame/space';
import type { Gpu } from '../gpu/device';
import { wgsl } from '../gpu/wgsl';
import type { Labs } from '../labs';
import type { Region, Rules } from '../rules/regions';
import type { GroundSource } from '../stream/ground';
import type { GroundPage } from '../stream/ground-pages';
import { PAGE } from '../stream/heights';
import { COARSEST, key, MAP, type Node } from '../terrain/lod';
import { dayOfYear, mosaicsAt, packRulesOn, slots } from './rules';

/** Ground pages: whole layers of SIDE × SIDE pages, as GROUND_SIDE in look.wgsl. */
const SIDE = 4, LAYER_PAGES = SIDE * SIDE, POOL = 128;
/** Pages uploaded per frame at most, material tiles in flight, and pages whose openness follows new heights per frame. */
const ARRIVALS = 8, LOADS = 4, REFRESHES = 8;
const JOB_BYTES = 32;
/** The look at the eye blends the regions of points this far apart about it, with a tent weight. */
const LOOK_STEP_M = 1000, LOOK_REACH = 2;
const RAD = Math.PI / 180;

/** Without a ruleset, the ground is the palette under the light and haze of the prototype's `world` region. */
const NEUTRAL: Rules = {
    vocabulary: { materials: [{ id: 'none', cover: 'grass' }], lithologies: [{ id: 'unknown', hardness: 0.5 }] },
    regions: [{
        id: 'world', chain: ['world'], leaf: 'mixed', look: {
            snap: 0.4, hazeKm: 35, hazeHeight: 2500, mistTop: 0, softness: 0.08, gradient: 0.35, ramps: {}, outcrops: 0, snow_share: 0.5, turf_deg: 42,
            rock: '#8c877d', sea: { deep: '#2c5874', shallow: '#4f8088' }, grade: { shadow: [-0.004, 0, 0.012], light: [0.01, 0.004, -0.008], saturation: 1.08 },
        },
    }],
};

interface Slot {
    node: Node;
    slot: number;
    used: number;
    /** The date of the content, or -1 before the first upload. */
    date: number;
    /** The region of each texel, for the look at the eye. */
    regions?: Uint8Array;
}

export interface GroundStats { resident: number; capacity: number; wanted: number; shortage: number; colour: string; requests: number; bytes: number; failed: number }

interface Resources {
    gpu: Gpu;
    cells: GPUTexture;
    colour: GPUTexture;
    snow: GPUTexture;
    openness: GPUTexture;
    cache: GPUTexture;
    rules: GPUTexture;
    map: GPUBuffer;
    jobs: GPUBuffer;
    open: GPUBuffer;
    group: GPUBindGroup;
    computeGroups: [GPUBindGroup, GPUBindGroup];
    compute: GPUComputePipeline;
}

/**
 * The ground pages: per node of the levels with ground tiles, the ground on the date in a fixed pool,
 * found by the ground map in the windows of the LOD map. A node finer than the ground draws the ground
 * of its ancestor. A new date builds the pages again; they draw the old date until then.
 */
export class Ground {
    readonly rules: Rules;
    readonly finest: number;
    private layout?: { render: GPUBindGroupLayout; compute: GPUBindGroupLayout[] };
    private resources?: Resources;
    private surfaceGroup?: GPUBindGroup;
    /** Nodes whose material tile has no data there: a coarser page draws. */
    private readonly absent = new Set<string>();
    private readonly pages = new Map<string, Slot>();
    private free = range(POOL);
    private frame = 0;
    private date = -1;
    private dateKey = '';
    private loads = 0;
    /** Material tiles of the date in flight, or whose pages wait in `arrivals`, with the pages that wait. */
    private readonly loading = new Map<string, number>();
    private arrivals: { node: Node; page: GroundPage; date: number }[] = [];
    private jobs: { page: Slot; work: number }[] = [];
    private dirty = true;
    private origins: [number, number][] = [];
    private wanted = 0;
    private shortage = 0;
    /** Wanted pages without the content of the date. */
    private missing = 0;
    private readonly world: number;
    private today = { quarters: [0, 1] as [number, number], weight: 0, snowIndex: 0 };

    constructor(private readonly source: GroundSource | undefined, private readonly labs: Labs, private readonly changed: () => void) {
        this.rules = source?.rules ?? NEUTRAL;
        this.world = Math.max(0, this.rules.regions.findIndex(region => region.id === 'world'));
        this.finest = source?.finest ?? COARSEST;
    }

    get idle(): boolean {
        return this.missing === 0 && this.loads === 0 && this.arrivals.length === 0 && this.jobs.length === 0;
    }

    get stats(): GroundStats {
        let resident = 0;
        for (const page of this.pages.values()) if (page.date >= 0) resident++;
        const { requests = 0, bytes = 0, failed = 0 } = this.source ?? {};
        return { resident, capacity: POOL, wanted: this.wanted, shortage: this.shortage, colour: this.source?.colour ?? 'none', requests, bytes, failed };
    }

    get attribution(): string[] {
        return this.source?.attribution ?? [];
    }

    /** The bind group layouts: group 2 of the terrain and the sky, and groups 0 and 2 of the ground compute. */
    layouts(device: GPUDevice): { render: GPUBindGroupLayout; compute: GPUBindGroupLayout[] } {
        const stages = GPUShaderStage.FRAGMENT | GPUShaderStage.COMPUTE;
        const entries: GPUBindGroupLayoutEntry[] = [
            { binding: 0, visibility: stages, buffer: { type: 'uniform' } },
            { binding: 1, visibility: stages, buffer: { type: 'read-only-storage' } },
            { binding: 2, visibility: stages, texture: { sampleType: 'uint', viewDimension: '2d-array' } },
            ...[3, 4, 5].map(binding => ({ binding, visibility: stages, texture: { sampleType: 'float', viewDimension: '2d-array' } }) as GPUBindGroupLayoutEntry),
            { binding: 6, visibility: stages, texture: { sampleType: 'unfilterable-float' } },
            { binding: 7, visibility: stages, sampler: {} },
        ];
        const render = device.createBindGroupLayout({ entries: [...entries, { binding: 8, visibility: GPUShaderStage.FRAGMENT, texture: { sampleType: 'float', viewDimension: '2d-array' } }] });
        // The compute writes the cache: its bind group does not hold it for sampling.
        const compute = [
            device.createBindGroupLayout({ entries: [
                { binding: 0, visibility: GPUShaderStage.COMPUTE, buffer: { type: 'read-only-storage' } },
                { binding: 1, visibility: GPUShaderStage.COMPUTE, buffer: { type: 'storage' } },
                { binding: 2, visibility: GPUShaderStage.COMPUTE, storageTexture: { format: 'rgba8unorm', viewDimension: '2d-array' } },
            ] }),
            device.createBindGroupLayout({ entries }),
        ];
        this.layout = { render, compute };
        return this.layout;
    }

    /** The pools and the compute for a device, after `layouts`. A new device builds every page again. */
    async attach(gpu: Gpu, look: GPUBuffer, surface: { layout: GPUBindGroupLayout; group: GPUBindGroup }): Promise<void> {
        const { device } = gpu, layout = this.layout!, layers = POOL / LAYER_PAGES, side = PAGE * SIDE, cache = this.labs.ground === 'cache';
        this.resources = undefined;
        this.lost();
        const usage = GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST;
        const texture = (format: GPUTextureFormat, bytes: number, depth = layers, size = side, extra = 0) =>
            gpu.texture('ground', { size: [size, size, depth], format, usage: usage | extra }, size * size * depth * bytes);
        const SLOT = slots(this.rules.vocabulary);
        const r = {
            gpu,
            cells: texture('rgba8uint', 4), colour: texture('rgba8unorm', 4), snow: texture('rgba8unorm', 4), openness: texture('r8unorm', 1),
            cache: cache ? texture('rgba8unorm', 4, 4 * layers, side, GPUTextureUsage.STORAGE_BINDING) : texture('rgba8unorm', 4, 4, 1, GPUTextureUsage.STORAGE_BINDING),
            rules: gpu.texture('ground', { size: [SLOT.count, this.rules.regions.length], format: 'rgba32float', usage }, SLOT.count * this.rules.regions.length * 16),
            map: gpu.buffer('ground', (this.finest + 1) * MAP * MAP * 4, GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST),
            jobs: gpu.buffer('compute', (ARRIVALS + REFRESHES) * JOB_BYTES, GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST),
            open: gpu.buffer('compute', (ARRIVALS + REFRESHES) * PAGE * PAGE, GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC),
        };
        const sampler = device.createSampler({ magFilter: 'linear', minFilter: 'linear' });
        const view = (t: GPUTexture) => t.createView({ dimension: '2d-array' });
        const inputs: GPUBindGroupEntry[] = [
            { binding: 0, resource: { buffer: look } }, { binding: 1, resource: { buffer: r.map } }, { binding: 2, resource: view(r.cells) },
            { binding: 3, resource: view(r.colour) }, { binding: 4, resource: view(r.snow) }, { binding: 5, resource: view(r.openness) },
            { binding: 6, resource: r.rules.createView() }, { binding: 7, resource: sampler },
        ];
        const module = device.createShaderModule({ label: 'ground', code: wgsl('ground/ground.wgsl') });
        const compute = await device.createComputePipelineAsync({
            label: 'ground', layout: device.createPipelineLayout({ bindGroupLayouts: [layout.compute[0], surface.layout, layout.compute[1]] }),
            compute: { module, entryPoint: 'ground_main', constants: surfaceConstants(this.labs) },
        });
        this.resources = {
            ...r, compute,
            group: device.createBindGroup({ layout: layout.render, entries: [...inputs, { binding: 8, resource: view(r.cache) }] }),
            computeGroups: [
                device.createBindGroup({ layout: layout.compute[0], entries: [
                    { binding: 0, resource: { buffer: r.jobs } }, { binding: 1, resource: { buffer: r.open } }, { binding: 2, resource: view(r.cache) },
                ] }),
                device.createBindGroup({ layout: layout.compute[1], entries: inputs }),
            ],
        };
        this.surfaceGroup = surface.group;
        this.dateKey = '';
    }

    /** Group 2 of the terrain and the sky. */
    get group(): GPUBindGroup | undefined {
        return this.resources?.group;
    }

    /**
     * Takes the pages that the drawn nodes and the selection want, near and drawn first, on the date of
     * the view. `refreshed` are the nodes whose heights changed, so their openness follows.
     */
    update(drawn: Node[], wanted: Node[], refreshed: Node[], origins: [number, number][], date: string): void {
        const r = this.resources;
        if (!r) return;
        this.frame++;
        this.setDate(r, date);
        if (!this.source) return;
        const ground = (node: Node): Node => {
            const level = Math.min(node.level, this.finest), shift = node.level - level;
            return { level, x: node.x >> shift, y: node.y >> shift };
        };
        const order = new Map<string, Node>();
        for (const node of [...wanted.filter(n => n.level <= 8), ...drawn, ...wanted].map(ground)) {
            if (!order.has(key(node)) && !this.absent.has(key(node))) order.set(key(node), node);
        }
        this.wanted = order.size;
        this.shortage = 0;
        this.missing = 0;
        for (const node of order.values()) {
            const page = this.pages.get(key(node));
            if (page) page.used = this.frame;
        }
        for (const node of order.values()) {
            let page = this.pages.get(key(node));
            if (!page) {
                page = this.allocate(node);
                if (!page) {
                    this.shortage++;
                    continue;
                }
            }
            if (page.date !== this.date) {
                this.missing++;
                this.load(node);
            }
        }
        let uploads = 0;
        while (uploads < ARRIVALS && this.arrivals.length) {
            const { node, page: data, date } = this.arrivals.shift()!;
            this.settle(tileOf(node, date));
            const page = this.pages.get(key(node));
            if (!page || date !== this.date) continue;
            this.upload(r, page, data);
            page.date = date;
            page.regions = Uint8Array.from({ length: PAGE * PAGE }, (_, i) => data.cells[4 * i + 2]);
            this.jobs.push({ page, work: this.labs.ground === 'cache' ? 3 : 1 });
            this.dirty = true;
            uploads++;
        }
        let refreshes = 0;
        for (const node of refreshed) {
            const page = this.pages.get(key(node));
            if (refreshes >= REFRESHES || !page || page.date < 0 || this.jobs.some(job => job.page === page)) continue;
            this.jobs.push({ page, work: 1 });
            refreshes++;
        }
        if (origins.some((origin, level) => origin[0] !== this.origins[level]?.[0] || origin[1] !== this.origins[level]?.[1])) this.dirty = true;
        this.origins = origins;
        if (this.dirty) r.gpu.device.queue.writeBuffer(r.map, 0, this.map());
        this.dirty = false;
    }

    /** Encodes the openness and the cache of the pages that arrived or whose heights changed, timed by `timing` when it runs. */
    encode(encoder: GPUCommandEncoder, timing?: () => GPUComputePassTimestampWrites | undefined): void {
        const r = this.resources, jobs = this.jobs.splice(0);
        if (!r || !jobs.length || !this.surfaceGroup) return;
        const data = new ArrayBuffer(jobs.length * JOB_BYTES), ints = new Int32Array(data), floats = new Float32Array(data);
        jobs.forEach(({ page: { node: { level, x, y }, slot }, work }, k) => {
            ints.set([x * PAGE, y * PAGE, level, slot], 8 * k);
            floats[8 * k + 4] = 2 * Math.PI * EARTH_RADIUS * Math.cos(tileLonLat(x + 0.5, y + 0.5, level)[1] * RAD) / 2 ** (level + 8);
            ints[8 * k + 5] = work;
        });
        r.gpu.device.queue.writeBuffer(r.jobs, 0, data);
        const pass = encoder.beginComputePass({ label: 'ground', timestampWrites: timing?.() });
        pass.setPipeline(r.compute);
        pass.setBindGroup(0, r.computeGroups[0]);
        pass.setBindGroup(1, this.surfaceGroup);
        pass.setBindGroup(2, r.computeGroups[1]);
        pass.dispatchWorkgroups(PAGE / 4 / 16, PAGE / 16, jobs.length);
        pass.end();
        jobs.forEach(({ page }, k) => encoder.copyBufferToTexture({ buffer: r.open, offset: k * PAGE * PAGE, bytesPerRow: PAGE }, { texture: r.openness, origin: origin(page.slot) }, [PAGE, PAGE, 1]));
    }

    /** A lost device keeps the tile bodies; every page comes again. */
    lost(): void {
        this.pages.clear();
        this.free = range(POOL);
        this.arrivals = [];
        this.loading.clear();
        this.jobs = [];
        this.dirty = true;
    }

    /**
     * The regions about a point and their weights, which sum to 1, from the finest resident page at each
     * of the points LOOK_STEP_M apart about it, with a tent weight.
     */
    regionsAt(lon: number, lat: number): Map<number, number> {
        const out = new Map<number, number>(), dlat = LOOK_STEP_M / (EARTH_RADIUS * RAD), dlon = dlat / Math.cos(lat * RAD);
        let total = 0;
        for (let j = -LOOK_REACH; j <= LOOK_REACH; j++) for (let i = -LOOK_REACH; i <= LOOK_REACH; i++) {
            const region = this.regionAt(lon + i * dlon, lat + j * dlat);
            if (region === undefined) continue;
            const w = (LOOK_REACH + 1 - Math.abs(i)) * (LOOK_REACH + 1 - Math.abs(j));
            out.set(region, (out.get(region) ?? 0) + w);
            total += w;
        }
        if (!total) return new Map([[this.world, 1]]);
        for (const [region, w] of out) out.set(region, w / total);
        return out;
    }

    private regionAt(lon: number, lat: number): number | undefined {
        const [ux, uy] = worldPixel([lon, lat], UNITS);
        for (let level = this.finest; level >= COARSEST; level--) {
            const tx = Math.floor(ux / 2 ** (22 - level)), ty = Math.floor(uy / 2 ** (22 - level));
            const page = this.pages.get(`${level}/${tx >> 8}/${ty >> 8}`);
            if (page?.regions) return page.regions[(ty & 255) * PAGE + (tx & 255)];
        }
        return undefined;
    }

    /** A new date: the rules of the date at once, and the pages again from the kept tiles. */
    private setDate(r: Resources, date: string): void {
        const day = dayOfYear(date), { quarters, weight } = mosaicsAt(day), snowIndex = seasonDay(date).index;
        const dateKey = `${quarters}/${weight.toFixed(3)}/${snowIndex}`;
        if (dateKey === this.dateKey) return;
        this.dateKey = dateKey;
        this.date++;
        this.today = { quarters, weight, snowIndex };
        const SLOT = slots(this.rules.vocabulary), rows = this.rules.regions.length;
        r.gpu.device.queue.writeTexture({ texture: r.rules }, packRulesOn(this.rules.regions, this.rules.vocabulary, day), { bytesPerRow: SLOT.count * 16 }, [SLOT.count, rows]);
    }

    /** Requests the four pages of the material tile of a node, on the current date. */
    private load(node: Node): void {
        const z = node.level - 1, x = node.x >> 1, y = node.y >> 1, name = tileOf(node, this.date);
        if (!this.source || this.loading.has(name) || this.loads >= LOADS) return;
        const date = this.date, { quarters, weight, snowIndex } = this.today;
        const shares = this.rules.regions.map((region: Region) => region.look.snow_share);
        this.loads++;
        this.loading.set(name, 1);
        this.source.pages(z, x, y, quarters, { weight, snowIndex, shares }, this.world).then(pages => {
            pages.forEach((page, q) => {
                const node = { level: z + 1, x: 2 * x + (q & 1), y: 2 * y + (q >> 1) };
                if (!page) return this.absent.add(key(node));
                this.arrivals.push({ node, page, date });
                this.loading.set(name, this.loading.get(name)! + 1);
            });
        }).catch(error => console.warn(`Ground tile ${z}/${x}/${y}: ${error instanceof Error ? error.message : error}`)).finally(() => {
            this.loads--;
            this.settle(name);
            this.changed();
        });
    }

    /** One page of a material tile in flight or waiting is done; the last frees the tile for a new load. */
    private settle(name: string): void {
        const left = (this.loading.get(name) ?? 1) - 1;
        if (left > 0) this.loading.set(name, left);
        else this.loading.delete(name);
    }

    /** A free slot, or the one that the frames wanted longest ago; none when this frame wants them all. */
    private allocate(node: Node): Slot | undefined {
        let slot = this.free.pop();
        if (slot === undefined) {
            let oldest: Slot | undefined;
            for (const page of this.pages.values()) if (page.used < this.frame && (!oldest || page.used < oldest.used)) oldest = page;
            if (!oldest) return undefined;
            this.pages.delete(key(oldest.node));
            if (oldest.date >= 0) this.dirty = true;
            slot = oldest.slot;
        }
        const page: Slot = { node, slot, used: this.frame, date: -1 };
        this.pages.set(key(node), page);
        return page;
    }

    private upload(r: Resources, page: Slot, data: GroundPage): void {
        const { device } = r.gpu;
        for (const [texture, texels] of [[r.cells, data.cells], [r.colour, data.colour], [r.snow, data.snow]] as const) {
            device.queue.writeTexture({ texture, origin: origin(page.slot) }, texels, { bytesPerRow: 4 * PAGE }, [PAGE, PAGE, 1]);
        }
    }

    /** The ground map: MAP × MAP entries per level, in the windows of the LOD map. */
    private map(): Uint32Array<ArrayBuffer> {
        const map = new Uint32Array((this.finest + 1) * MAP * MAP);
        for (const page of this.pages.values()) {
            const { level, x, y } = page.node, window = this.origins[level];
            if (page.date < 0 || !window) continue;
            const i = x - window[0], j = y - window[1];
            if (i >= 0 && j >= 0 && i < MAP && j < MAP) map[(level * MAP + j) * MAP + i] = page.slot + 1;
        }
        return map;
    }
}

/** The overrides of drawn-height.wgsl, as the terrain sets them. */
export const surfaceConstants = (labs: Labs) => ({ RINGS: labs.terrain === 'rings' ? 1 : 0, R16: labs.pages === 'r16' ? 1 : 0 });

/** Where a ground page lies: one of SIDE × SIDE tiles of a layer, as `in_page` in look.wgsl. */
const origin = (slot: number): [number, number, number] => [PAGE * (slot % SIDE), PAGE * (Math.floor(slot / SIDE) % SIDE), Math.floor(slot / LAYER_PAGES)];

const range = (n: number) => Array.from({ length: n }, (_, i) => n - 1 - i);

/** The material tile of a node, on a date. */
const tileOf = ({ level, x, y }: Node, date: number) => `${level - 1}/${x >> 1}/${y >> 1}/${date}`;
