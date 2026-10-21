import { EARTH_RADIUS } from '../frame/frame';
import type { Anchor } from '../frame/space';
import { PAGE, type PageData } from '../stream/heights';
import { TILE_ZOOM, type TerrainSource } from '../stream/terrain';
import { NEAR, type NodeStamps } from './bins';
import { COARSEST, FINEST, key, MAP, size, type Node } from './lod';

/** Tile requests in flight at most. */
const LOADS = 6;
/** Pages that arrive or are synthesized per frame at most; each gets its normals in the same frame. */
export const ARRIVALS = 16;
/** Words of stamp data per frame at most: the nodes of the frame's stamps. */
export const STAMP_WORDS = 2 ** 20;
/** Pages whose normals are computed again per frame at most, after a neighbour arrived. */
const REFRESHES = 8;
/** Encoded tile bodies kept, so a new device gets the pages again without a request. */
const BODIES = 64 * 2 ** 20;
/** The modes of a stamp job, as in stamp.wgsl. */
export const MODE = { HEIGHTS: 1, MASK: 4 } as const;
/**
 * Half a texel of a level at the equator, in metres: a summit inside a texel with slopes of 1 rises
 * at most this far above the texel's mean, so the finer pages below a page keep to its range widened by it.
 */
const summit = (level: number) => Math.PI * EARTH_RADIUS / 2 ** level / PAGE;

export type Kind = 'stored' | 'synth';

export interface Page {
    node: Node;
    kind: Kind;
    /** The layer in the page arrays. */
    slot: number;
    /** The last frame that wanted the page. */
    used: number;
    /** Its known heights and those of the finer pages below it; a synthesized page has the range of its stored ancestor. */
    range: [number, number];
    /** Whether the page knows every height; where it does not, the next coarser page draws. */
    complete: boolean;
    /** The heights are on the GPU: raw for a stored page, stamped for a synthesized one. */
    ready: boolean;
    /** The layer that draws the page once its stamps are on: its own, or the stamped copy of a stored page. */
    drawn?: number;
    copy?: number;
    mask?: number;
}

/** What stamps a node: the binned features, while they are known. */
export interface StampSource {
    get(node: Node): NodeStamps | undefined;
}

const NO_STAMPS: StampSource = { get: () => ({ data: null, heights: false }) };

/**
 * A stamp job: its page's heights are in `out` (the synthesized base, or none for a stored page,
 * which reads its raw layer) and go to the layer `target`.
 */
export interface Stamp {
    page: Page;
    out: number;
    target: number;
    data: Uint32Array<ArrayBuffer> | null;
    mode: number;
}

/** The GPU work of one frame, in order: uploads and syntheses, the stamps, then the normals, then the LOD map. */
export interface Work {
    uploads: { page: Page; data: PageData }[];
    /** Each with the level of its stored ancestor; the synthesis of page k goes to `out` page k. */
    syntheses: { page: Page; stored: number }[];
    /** The stamps of the syntheses, then those of stored pages, whose heights go to `out` after the syntheses (`copies`). */
    stamps: Stamp[];
    normals: Page[];
    /** The LOD map: MAP × MAP entries per level from 0 to FINEST, when it changed. */
    map?: Uint32Array<ArrayBuffer>;
    /** Per layer, the mask layer + 1 (0 for none), when they changed. */
    masks?: Uint32Array<ArrayBuffer>;
    /** The node of each level at the LOD map window's origin. */
    origins: [number, number][];
}

/** The stamps that write the heights of a stored page: they go to `out` after the syntheses. */
export const copies = (work: Work): Stamp[] => work.stamps.filter(({ page, mode }) => page.kind === 'stored' && mode & MODE.HEIGHTS);

interface Tile {
    state: 'loading' | 'decoding' | 'done' | 'absent';
    /** Quadrants without a known height, once decoded. */
    empty?: boolean[];
}

/**
 * The page pools and what fills them. Stored pages come from the stored tiles, synthesized pages from
 * their deepest stored ancestor. The pools have a fixed size: the page that the frames wanted longest
 * ago leaves first, and a frame that wants more pages than a pool holds counts a shortage. Mask
 * layers go to the finest pages first.
 */
export class Pages {
    requests = 0;
    bytes = 0;
    failed = 0;
    /** Wanted pages that found no free slot in the last frame. */
    shortage = 0;
    /** Wanted pages that are not ready yet in the last frame. */
    missing = 0;
    /** Pages of each pool that the last frame wanted. */
    wanted: Record<Kind, number> = { stored: 0, synth: 0 };
    readonly pages = new Map<string, Page>();
    private readonly tiles = new Map<string, Tile>();
    private readonly bodies = new Map<string, ArrayBuffer>();
    private bodyBytes = 0;
    private readonly free: Record<Kind, number[]>;
    private maskFree: number[];
    private arrivals: { node: Node; data: PageData }[] = [];
    /** Tiles whose decoded pages wait in `arrivals`, by name. */
    private readonly queued = new Map<string, number>();
    private readonly refresh = new Set<Page>();
    private frame = 0;
    /** Tile loads and decodes in flight. */
    loads = 0;
    private dirty = true;
    private origins: [number, number][] = [];

    constructor(private readonly source: TerrainSource, readonly capacity: Record<Kind, number>, private readonly bounds: [number, number, number, number], private readonly changed: () => void,
        private readonly stamps: StampSource = NO_STAMPS, readonly maskCapacity = 0) {
        this.free = { stored: range(0, capacity.stored), synth: range(capacity.stored, capacity.stored + capacity.synth) };
        this.maskFree = range(0, maskCapacity);
    }

    get idle(): boolean {
        return this.missing === 0 && this.loads === 0 && this.arrivals.length === 0 && this.refresh.size === 0;
    }

    /**
     * The heights that a node can draw: the range of the finest page that covers it, with the ranges
     * of the coarser pages that draw where it knows no height. Undefined while no page covers it.
     */
    range(node: Node): [number, number] | undefined {
        let out: [number, number] | undefined;
        for (let { level, x, y } = node; level >= COARSEST; level--, x >>= 1, y >>= 1) {
            const page = this.pages.get(`${level}/${x}/${y}`);
            if (!page) continue;
            out = out ? [Math.min(out[0], page.range[0]), Math.max(out[1], page.range[1])] : page.range;
            if (page.complete) return out;
        }
        return out;
    }

    /**
     * Whether a page that draws holds a node, at its level or a coarser one. If none does, the node draws
     * nothing: each bilinear quad that it reads has a texel in its own page of that level.
     */
    covers(node: Node): boolean {
        return this.drawing(node) !== undefined;
    }

    /** The node of the finest page that draws and holds the node: the heights that it draws. */
    drawing(node: Node): Node | undefined {
        for (let { level, x, y } = node; level >= COARSEST; level--, x >>= 1, y >>= 1) if (this.pages.get(`${level}/${x}/${y}`)?.drawn !== undefined) return { level, x, y };
        return undefined;
    }

    /** Takes the pages that a frame wants, coarse and near first, and returns its GPU work. */
    update(wanted: Node[], anchor: Anchor): Work {
        this.frame++;
        this.shortage = 0;
        this.missing = 0;
        this.wanted = { stored: 0, synth: 0 };
        const work: Work = { uploads: [], syntheses: [], stamps: [], normals: [], origins: this.window(anchor) };
        // Every wanted page is used before the first allocation, so none leaves for a later one of this frame.
        for (const node of wanted) {
            const page = this.pages.get(key(node));
            if (page) page.used = this.frame;
        }
        for (const node of wanted) this.want(node, work);
        const keys = new Set(wanted.map(key));
        while (work.uploads.length + work.syntheses.length < ARRIVALS && this.arrivals.length) {
            const { node, data } = this.arrivals.shift()!;
            const tile = `${node.level - 1}/${node.x >> 1}/${node.y >> 1}`, heights: [number, number] = [data.min - summit(node.level), data.max + summit(node.level)];
            this.queued.set(tile, this.queued.get(tile)! - 1);
            const page = this.pages.get(key(node)) ?? this.allocate(node, 'stored', heights, keys.has(key(node)));
            if (!page || page.ready) continue;
            page.range = heights;
            page.complete = data.complete;
            page.ready = true;
            work.uploads.push({ page, data });
        }
        // A stored page draws once its stamps are on: as it is, or as a stamped copy.
        const fresh = work.syntheses.map(({ page }) => page);
        for (const page of fresh) page.ready = true;
        for (const page of this.pages.values()) {
            // The fresh pages of a frame take its normals jobs: at most ARRIVALS of them, and the refreshes.
            if (fresh.length < ARRIVALS && page.kind === 'stored' && page.ready && page.drawn === undefined && page.used === this.frame && this.resolve(page, work)) fresh.push(page);
        }
        for (const page of fresh) {
            this.dirty = true;
            work.normals.push(page);
            this.refresh.delete(page);
            for (const neighbour of this.neighbours(page.node)) if (neighbour.drawn !== undefined) this.refresh.add(neighbour);
        }
        for (const page of [...this.refresh].slice(0, REFRESHES)) {
            this.refresh.delete(page);
            if (page.drawn !== undefined && !work.normals.includes(page)) work.normals.push(page);
        }
        if (this.dirty) {
            work.map = this.map();
            work.masks = this.masks();
        }
        this.dirty = false;
        return work;
    }

    /** A lost device keeps the tiles that were absent and their bodies; every page comes again. */
    lost(): void {
        this.pages.clear();
        this.maskFree = range(0, this.maskCapacity);
        this.arrivals = [];
        this.queued.clear();
        this.refresh.clear();
        this.free.stored = range(0, this.capacity.stored);
        this.free.synth = range(this.capacity.stored, this.capacity.stored + this.capacity.synth);
        for (const tile of this.tiles.values()) if (tile.state === 'decoding') tile.state = 'done';
        this.dirty = true;
    }

    private want(node: Node, work: Work): void {
        const page = this.pages.get(key(node));
        if (page) {
            page.used = this.frame;
            this.wanted[page.kind]++;
            if (page.drawn === undefined) this.missing++;
            return;
        }
        const stored = this.stored(node);
        if (stored !== 'no') {
            this.wanted.stored++;
            this.missing++;
            this.load(node.level - 1, node.x >> 1, node.y >> 1);
            return;
        }
        const ancestor = this.storedAncestor(node);
        // No stored tile above the node knows a height: nothing draws there.
        if (!ancestor) return;
        this.wanted.synth++;
        this.missing++;
        if (ancestor === 'wait') return;
        // The synthesis reads the ancestor and its neighbours: no allocation of this frame takes them.
        const inputs = this.around(ancestor).map(near => {
            const page = this.pages.get(key(near));
            if (page) page.used = this.frame;
            return { near, page };
        });
        const ready = inputs.every(({ near, page }) => {
            if (page?.ready || this.stored(near) === 'no') return true;
            if (!page) this.load(near.level - 1, near.x >> 1, near.y >> 1);
            return false;
        });
        if (!ready) return;
        const stamps = this.stamps.get(node);
        if (!stamps || this.busy(work, stamps.data?.length)) return;
        const synth = this.allocate(node, 'synth', this.range(ancestor)!, true);
        if (!synth) return;
        synth.drawn = synth.slot;
        if (stamps.data) {
            const mode = (stamps.heights ? MODE.HEIGHTS : 0) | (node.level >= NEAR && this.mask(synth) ? MODE.MASK : 0);
            if (mode) work.stamps.push({ page: synth, out: work.syntheses.length, target: synth.slot, data: stamps.data, mode });
        }
        work.syntheses.push({ page: synth, stored: ancestor.level });
    }

    /** Whether the frame has no room for another arrival, synthesis or stamp, or for `words` of stamp data. */
    private busy(work: Work, words = 0): boolean {
        const used = work.stamps.reduce((sum, { data }) => sum + (data?.length ?? 0), 0);
        return work.uploads.length + work.syntheses.length + copies(work).length >= ARRIVALS || used + words > STAMP_WORDS;
    }

    /**
     * Puts the stamps on a ready stored page once its features are known: none, or a stamped copy that
     * draws while the raw page stays for the syntheses.
     */
    private resolve(page: Page, work: Work): boolean {
        const stamps = this.stamps.get(page.node);
        if (!stamps) return false;
        if (!stamps.data || !stamps.heights) {
            page.drawn = page.slot;
            return true;
        }
        if (this.busy(work, stamps.data.length)) return false;
        const copy = this.slot('stored');
        if (copy === undefined) {
            this.shortage++;
            return false;
        }
        page.copy = page.drawn = copy;
        work.stamps.push({ page, out: work.syntheses.length + copies(work).length, target: copy, data: stamps.data, mode: MODE.HEIGHTS });
        return true;
    }

    /**
     * A mask layer for a page: a free one, or the one of the page that the frames wanted longest ago,
     * or else that of a coarser page of this frame: the finest pages, near the eye, keep theirs.
     */
    private mask(page: Page): boolean {
        let slot = this.maskFree.pop();
        if (slot === undefined) {
            const rank = (other: Page) => (other.used < this.frame ? other.used - this.frame - FINEST : other.node.level - page.node.level);
            let donor: Page | undefined;
            for (const other of this.pages.values()) if (other.mask !== undefined && rank(other) < 0 && (!donor || rank(other) < rank(donor))) donor = other;
            if (!donor) return false;
            slot = donor.mask;
            donor.mask = undefined;
        }
        page.mask = slot;
        this.dirty = true;
        return true;
    }

    /**
     * Whether a node has a stored page: `yes`, `no` (its tile is absent or knows no height there, so
     * a stored ancestor gives its page) or `unknown` (its tile has not arrived).
     */
    private stored({ level, x, y }: Node): 'yes' | 'no' | 'unknown' {
        if (level - 1 > TILE_ZOOM) return 'no';
        const tile = this.tiles.get(`${level - 1}/${x >> 1}/${y >> 1}`);
        if (tile?.state === 'absent' || tile?.empty?.[(x & 1) + 2 * (y & 1)]) return 'no';
        return tile?.state === 'done' || tile?.empty ? 'yes' : 'unknown';
    }

    /** The deepest ancestor with a stored page once it is ready, `wait` before, and none where no tile above knows a height. */
    private storedAncestor(node: Node): Node | 'wait' | undefined {
        for (let { level, x, y } = node; level > COARSEST;) {
            level--; x >>= 1; y >>= 1;
            const parent = { level, x, y }, state = this.stored(parent);
            if (state === 'no') continue;
            const page = this.pages.get(key(parent));
            if (page) page.used = this.frame;
            if (state === 'yes' && page?.ready) return parent;
            this.load(level - 1, x >> 1, y >> 1);
            return 'wait';
        }
        return undefined;
    }

    /** A node and its eight neighbours inside the bounds. */
    private around({ level, x, y }: Node): Node[] {
        const s = size(level), out: Node[] = [];
        for (let dy = -1; dy <= 1; dy++) for (let dx = -1; dx <= 1; dx++) {
            const nx = x + dx, ny = y + dy;
            if (nx < 0 || ny < 0 || nx >= 2 ** level || ny >= 2 ** level) continue;
            if (nx * s >= this.bounds[2] || (nx + 1) * s <= this.bounds[0] || ny * s >= this.bounds[3] || (ny + 1) * s <= this.bounds[1]) continue;
            out.push({ level, x: nx, y: ny });
        }
        return out;
    }

    private neighbours(node: Node): Page[] {
        return this.around(node).flatMap(near => this.pages.get(key(near)) ?? []).filter(page => page.node !== node);
    }

    /** Requests a stored tile, or decodes its kept body again. */
    private load(z: number, x: number, y: number): void {
        const name = `${z}/${x}/${y}`, tile = this.tiles.get(name);
        if ((tile && tile.state !== 'done') || this.queued.get(name)) return;
        if (this.loads >= LOADS) return;
        const body = this.bodies.get(name);
        this.loads++;
        this.tiles.set(name, { state: body ? 'decoding' : 'loading', empty: tile?.empty });
        (async () => {
            let data = body;
            if (!data) {
                this.requests++;
                data = await this.source.fetch(z, x, y);
                if (!data) return this.tiles.set(name, { state: 'absent' });
                this.bytes += data.byteLength;
                this.keep(name, data);
            }
            const quadrants = await this.source.decode(data);
            this.tiles.set(name, { state: 'done', empty: quadrants.map(quadrant => !quadrant) });
            quadrants.forEach((data, q) => {
                if (data) this.arrivals.push({ node: { level: z + 1, x: 2 * x + (q & 1), y: 2 * y + (q >> 1) }, data });
            });
            this.queued.set(name, quadrants.filter(Boolean).length);
        })().catch(error => {
            // A tile that fails stays out for this page, as an absent one.
            console.warn(`Terrain tile ${name}: ${error instanceof Error ? error.message : error}`);
            this.failed++;
            this.tiles.set(name, { state: 'absent' });
        }).finally(() => {
            this.loads--;
            this.changed();
        });
    }

    private keep(name: string, body: ArrayBuffer): void {
        this.bodies.set(name, body);
        this.bodyBytes += body.byteLength;
        for (const [old, kept] of this.bodies) {
            if (this.bodyBytes <= BODIES) break;
            this.bodies.delete(old);
            this.bodyBytes -= kept.byteLength;
        }
    }

    /**
     * A slot for a page: a free one, or the one that the frames wanted longest ago. None when this
     * frame wants them all: a shortage when the frame wants the new page too.
     */
    private allocate(node: Node, kind: Kind, heights: [number, number], wanted: boolean): Page | undefined {
        const slot = this.slot(kind);
        if (slot === undefined) {
            if (wanted) this.shortage++;
            return undefined;
        }
        // A synthesized page reads its stored ancestors only, and `heights` holds all that they draw.
        const page: Page = { node, kind, slot, used: this.frame, range: heights, complete: true, ready: false };
        this.pages.set(key(node), page);
        return page;
    }

    /** A free layer of a pool, or that of the page that the frames wanted longest ago, which leaves with its copy and mask. */
    private slot(kind: Kind): number | undefined {
        const free = this.free[kind].pop();
        if (free !== undefined) return free;
        let oldest: Page | undefined;
        for (const page of this.pages.values()) if (page.kind === kind && page.used < this.frame && (!oldest || page.used < oldest.used)) oldest = page;
        if (!oldest) return undefined;
        this.pages.delete(key(oldest.node));
        this.refresh.delete(oldest);
        if (oldest.ready) this.dirty = true;
        if (oldest.copy !== undefined) this.free.stored.push(oldest.copy);
        if (oldest.mask !== undefined) this.maskFree.push(oldest.mask);
        return oldest.slot;
    }

    /** The LOD map window of each level: MAP × MAP nodes about the eye. */
    private window(anchor: Anchor): [number, number][] {
        const origins = Array.from({ length: FINEST + 1 }, (_, level): [number, number] => [
            Math.floor((anchor.whole[0] + anchor.part[0]) / size(level)) - MAP / 2,
            Math.floor((anchor.whole[1] + anchor.part[1]) / size(level)) - MAP / 2,
        ]);
        if (origins.some((origin, level) => origin[0] !== this.origins[level]?.[0] || origin[1] !== this.origins[level]?.[1])) this.dirty = true;
        this.origins = origins;
        return origins;
    }

    /** Per entry, the layer + 1 that draws in the low 16 bits and the raw stored layer + 1 in the high 16 bits. */
    private map(): Uint32Array<ArrayBuffer> {
        const map = new Uint32Array((FINEST + 1) * MAP * MAP);
        for (const page of this.pages.values()) {
            if (!page.ready) continue;
            const { level, x, y } = page.node, [ox, oy] = this.origins[level], i = x - ox, j = y - oy;
            if (i < 0 || j < 0 || i >= MAP || j >= MAP) continue;
            map[(level * MAP + j) * MAP + i] = (page.drawn === undefined ? 0 : page.drawn + 1) | (page.kind === 'stored' ? (page.slot + 1) << 16 : 0);
        }
        return map;
    }

    private masks(): Uint32Array<ArrayBuffer> {
        const masks = new Uint32Array(this.capacity.stored + this.capacity.synth);
        for (const page of this.pages.values()) if (page.drawn !== undefined && page.mask !== undefined) masks[page.drawn] = page.mask + 1;
        return masks;
    }
}

const range = (from: number, to: number) => Array.from({ length: to - from }, (_, i) => to - 1 - i);
