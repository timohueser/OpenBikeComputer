import { worldPixel } from '../../planner/layers/mercator';
import { seasonDay } from '../../planner/layers/snow';
import { anchorAt, UNITS } from '../frame/space';
import type { GroundDate, GroundPage } from '../stream/ground-pages';
import { PAGE } from '../stream/heights';
import { COARSEST, key, MAP, size, type Node } from '../terrain/lod';
import { dayOfYear, mosaicsAt } from './rules';

/** Pages taken in per frame at most, material tiles in flight, and pages whose openness follows new heights per frame. */
const ARRIVALS = 8, LOADS = 4, REFRESHES = 8;
/** Levels that every view keeps: the ground under every other page while its own loads. */
const COARSE = 8;
/** The cover of a drawn page lies this many levels above it, never coarser than the coloured levels: what a short pool draws in its place. */
const COVER = 2;
/** The distance factor of a resident page in the order of the pool. */
const STAY = 0.7;
/** The look at the eye blends the regions at the nodes of this level about it (0.84 km at 47°), with a tent weight of this reach in nodes. */
const LOOK_LEVEL = 15, LOOK_REACH = 3;

/** The ground pages of material tile z/x/y on a date; null where it has no ground. */
export interface GroundTiles {
    /** The coarsest level whose pages carry the satellite colour. */
    readonly coloured?: number;
    pages(z: number, x: number, y: number, quarters: [number, number], date: GroundDate, world: number): Promise<(GroundPage | null)[]>;
}

export interface Slot {
    node: Node;
    /** The page in the ground textures. */
    slot: number;
    used: number;
    /** The date of the content, or -1 before the first upload. */
    date: number;
    /** The region of each texel, for the look at the eye. */
    regions?: Uint8Array;
}

/** The GPU work of a frame. */
export interface GroundWork {
    uploads: { page: Slot; data: GroundPage }[];
    /** Resident pages whose heights changed: their openness follows. */
    refreshes: Slot[];
    /** The ground map, when it changed: MAP × MAP entries per level in the windows of the LOD map, the slot + 1 or 0. */
    map?: Uint32Array<ArrayBuffer>;
}

/**
 * The ground pages without the GPU: per node of the levels with ground tiles, the ground on the date
 * in a pool of `capacity` slots. The coarse levels, then the cover of the drawn nodes, then the drawn
 * nodes and their morph parents nearest first, then the rest of the selection take the slots, so a
 * full pool leaves out the farthest and least needed pages, and their cover draws in their place. A
 * page takes its slot when its content arrives, so the page it replaces draws until then. A node
 * finer than the ground draws the ground of its ancestor; a tile that fails stays out for the page's
 * life, as an absent one.
 */
export class GroundPages {
    wanted = 0;
    /** Wanted pages that the pool has no slot for. */
    shortage = 0;
    /** Bumps on each new date: the ground of a slot is current when its `date` equals it. */
    date = -1;
    day = 0;
    readonly pages = new Map<string, Slot>();
    private readonly absent = new Set<string>();
    private free: number[];
    private frame = 0;
    private dateKey = '';
    private today = { quarters: [0, 1] as [number, number], weight: 0, snowIndex: 0 };
    private loads = 0;
    /** Material tiles of the date in flight, or whose pages wait in `arrivals`, with the pages that wait. */
    private readonly loading = new Map<string, number>();
    private arrivals: { node: Node; page: GroundPage; date: number }[] = [];
    private readonly refresh = new Set<Slot>();
    /** Wanted pages without the content of the date. */
    private missing = 0;
    private dirty = true;
    private kept = new Set<string>();
    private origins: [number, number][] = [];

    constructor(
        private readonly source: GroundTiles | undefined, readonly capacity: number, readonly finest: number,
        private readonly shares: number[], private readonly world: number, private readonly changed: () => void,
    ) {
        this.free = range(capacity);
    }

    get idle(): boolean {
        return this.missing === 0 && this.loads === 0 && this.arrivals.length === 0 && this.refresh.size === 0;
    }

    get resident(): number {
        let n = 0;
        for (const page of this.pages.values()) if (page.date >= 0) n++;
        return n;
    }

    /** Sets the date of the view; true when it is a new one, whose rules the GPU then needs. */
    setDate(date: string): boolean {
        const day = dayOfYear(date), { quarters, weight } = mosaicsAt(day), snowIndex = seasonDay(date).index;
        const dateKey = `${quarters}/${weight.toFixed(3)}/${snowIndex}`;
        if (dateKey === this.dateKey) return false;
        this.dateKey = dateKey;
        this.date++;
        this.day = day;
        this.today = { quarters, weight, snowIndex };
        return true;
    }

    /**
     * Takes the pages that the drawn nodes and the selection want, about the eye (lon, lat).
     * `refreshed` are the nodes whose heights changed.
     */
    update(drawn: Node[], wanted: Node[], refreshed: Node[], origins: [number, number][], eye: [number, number]): GroundWork {
        this.frame++;
        const work: GroundWork = { uploads: [], refreshes: [] };
        if (!this.source) return work;
        const anchor = anchorAt(...eye), ex = anchor.whole[0] + anchor.part[0], ey = anchor.whole[1] + anchor.part[1];
        // A resident page counts as nearer, so a page at the edge of a full pool does not leave and come back as the eye moves.
        const far = (node: Node) => Math.hypot((node.x + 0.5) * size(node.level) - ex, (node.y + 0.5) * size(node.level) - ey) * (this.pages.has(key(node)) ? STAY : 1);
        const keys = new Set(wanted.map(key));
        const near = drawn.flatMap(node => {
            const parent = { level: node.level - 1, x: node.x >> 1, y: node.y >> 1 };
            return keys.has(key(parent)) ? [node, parent] : [node];
        }).map(this.ground).sort((a, b) => far(a) - far(b));
        const coloured = this.source.coloured ?? COARSEST;
        const cover = drawn.map(this.ground).map(node => {
            const shift = node.level - Math.max(node.level - COVER, Math.min(coloured, node.level));
            return { level: node.level - shift, x: node.x >> shift, y: node.y >> shift };
        }).sort((a, b) => far(a) - far(b));
        const order = new Map<string, Node>();
        for (const node of [...wanted.filter(n => n.level <= COARSE).map(this.ground), ...cover, ...near, ...wanted.map(this.ground)]) {
            if (!order.has(key(node)) && !this.absent.has(key(node))) order.set(key(node), node);
        }
        const kept = [...order.values()].slice(0, this.capacity);
        this.wanted = order.size;
        this.shortage = order.size - kept.length;
        this.missing = 0;
        // Every kept page is used before the first allocation, so none leaves for a later one of this frame.
        this.kept = new Set(kept.map(key));
        for (const node of kept) {
            const page = this.pages.get(key(node));
            if (page) page.used = this.frame;
            if (page?.date !== this.date) {
                this.missing++;
                this.load(node);
            }
        }
        while (work.uploads.length < ARRIVALS && this.arrivals.length) {
            const { node, page: data, date } = this.arrivals.shift()!;
            this.settle(tileOf(node, date));
            if (date !== this.date || !this.kept.has(key(node))) continue;
            const page = this.pages.get(key(node)) ?? this.allocate(node);
            if (!page) continue;
            page.date = date;
            page.regions = Uint8Array.from({ length: PAGE * PAGE }, (_, i) => data.cells[4 * i + 2]);
            this.refresh.delete(page);
            work.uploads.push({ page, data });
            this.dirty = true;
        }
        const uploaded = new Set(work.uploads.map(upload => upload.page));
        for (const node of refreshed) {
            const page = this.pages.get(key(node));
            if (page && page.date >= 0 && !uploaded.has(page)) this.refresh.add(page);
        }
        for (const page of [...this.refresh].slice(0, REFRESHES)) {
            this.refresh.delete(page);
            if (this.pages.get(key(page.node)) === page) work.refreshes.push(page);
        }
        if (origins.some((origin, level) => origin[0] !== this.origins[level]?.[0] || origin[1] !== this.origins[level]?.[1])) this.dirty = true;
        this.origins = origins;
        if (this.dirty) work.map = this.map();
        this.dirty = false;
        return work;
    }

    /** A lost device keeps the absent tiles; every page and the rules come again. */
    lost(): void {
        this.pages.clear();
        this.free = range(this.capacity);
        this.arrivals = [];
        this.loading.clear();
        this.refresh.clear();
        this.dateKey = '';
        this.dirty = true;
    }

    /**
     * The regions about a point and their weights, which sum to 1: the finest resident page at the
     * nodes of LOOK_LEVEL about it, each with a tent weight of its distance. The nodes are fixed to the
     * world, so a weight changes smoothly as the point moves.
     */
    regionsAt(lon: number, lat: number): Map<number, number> {
        const [ux, uy] = worldPixel([lon, lat], UNITS), step = size(LOOK_LEVEL), cx = ux / step - 0.5, cy = uy / step - 0.5;
        const out = new Map<number, number>();
        let total = 0;
        for (let j = Math.ceil(cy - LOOK_REACH); j <= Math.floor(cy + LOOK_REACH); j++) {
            for (let i = Math.ceil(cx - LOOK_REACH); i <= Math.floor(cx + LOOK_REACH); i++) {
                const w = (LOOK_REACH - Math.abs(i - cx)) * (LOOK_REACH - Math.abs(j - cy));
                const region = w > 0 ? this.regionAt((i + 0.5) * step, (j + 0.5) * step) : undefined;
                if (region === undefined) continue;
                out.set(region, (out.get(region) ?? 0) + w);
                total += w;
            }
        }
        if (!total) return new Map([[this.world, 1]]);
        for (const [region, w] of out) out.set(region, w / total);
        return out;
    }

    private readonly ground = (node: Node): Node => {
        const level = Math.min(node.level, this.finest), shift = node.level - level;
        return { level, x: node.x >> shift, y: node.y >> shift };
    };

    private regionAt(ux: number, uy: number): number | undefined {
        for (let level = this.finest; level >= COARSEST; level--) {
            const tx = Math.floor(ux / 2 ** (22 - level)), ty = Math.floor(uy / 2 ** (22 - level));
            const page = this.pages.get(`${level}/${tx >> 8}/${ty >> 8}`);
            if (page?.regions) return page.regions[(ty & 255) * PAGE + (tx & 255)];
        }
        return undefined;
    }

    /** Requests the four pages of the material tile of a node, on the current date. */
    private load(node: Node): void {
        const z = node.level - 1, x = node.x >> 1, y = node.y >> 1, name = tileOf(node, this.date);
        if (!this.source || this.loading.has(name) || this.loads >= LOADS) return;
        const date = this.date, { quarters, weight, snowIndex } = this.today;
        const quarter = (q: number) => ({ level: z + 1, x: 2 * x + (q & 1), y: 2 * y + (q >> 1) });
        this.loads++;
        this.loading.set(name, 1);
        this.source.pages(z, x, y, quarters, { weight, snowIndex, shares: this.shares }, this.world).then(pages => {
            pages.forEach((page, q) => {
                if (!page) return this.absent.add(key(quarter(q)));
                this.arrivals.push({ node: quarter(q), page, date });
                this.loading.set(name, this.loading.get(name)! + 1);
            });
        }).catch(error => {
            console.warn(`Ground tile ${z}/${x}/${y}: ${error instanceof Error ? error.message : error}`);
            for (let q = 0; q < 4; q++) this.absent.add(key(quarter(q)));
        }).finally(() => {
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

    /** A free slot, or the one that the frames used longest ago; none when this frame keeps them all. */
    private allocate(node: Node): Slot | undefined {
        let slot = this.free.pop();
        if (slot === undefined) {
            let oldest: Slot | undefined;
            for (const page of this.pages.values()) if (page.used < this.frame && (!oldest || page.used < oldest.used)) oldest = page;
            if (!oldest) return undefined;
            this.pages.delete(key(oldest.node));
            this.refresh.delete(oldest);
            this.dirty = true;
            slot = oldest.slot;
        }
        const page: Slot = { node, slot, used: this.frame, date: -1 };
        this.pages.set(key(node), page);
        return page;
    }

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

const range = (n: number) => Array.from({ length: n }, (_, i) => n - 1 - i);

/** The material tile of a node, on a date. */
const tileOf = ({ level, x, y }: Node, date: number) => `${level - 1}/${x >> 1}/${y >> 1}/${date}`;
