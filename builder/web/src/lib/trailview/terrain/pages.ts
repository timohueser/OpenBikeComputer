import { EARTH_RADIUS } from '../frame/frame';
import type { Anchor } from '../frame/space';
import { PAGE, type PageData } from '../stream/heights';
import { TILE_ZOOM, type TerrainSource, stopped } from '../stream/terrain';
import { NEAR, type NodeStamps } from './bins';
import { COARSEST, distances, FINEST, key, MAP, size, type Node } from './lod';

/** Tile requests in flight at most. */
const LOADS = 6;
/** The level of the pages that fill the holes: one small tile of zoom 6 spans a view, so the view's own coarse tiles can come after the near field. */
const FILL = 7;
/** Pages that arrive or are synthesized per frame at most. */
export const ARRIVALS = 16;
/** Words of stamp data per frame at most: the nodes of the frame's stamps. */
export const STAMP_WORDS = 2 ** 20;
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

/** What stamps a node: the binned features, while they are known. `get` asks for them, `known` does not. */
export interface StampSource {
    get(node: Node): NodeStamps | undefined;
    known(node: Node): NodeStamps | undefined;
}

const NO_STAMPS: StampSource = { get: () => ({ data: null, heights: false, mask: false }), known: () => ({ data: null, heights: false, mask: false }) };

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

/** The GPU work of one frame, in order: uploads and syntheses, the stamps, then the LOD map. */
export interface Work {
    uploads: { page: Page; data: PageData }[];
    /** Each with the level of its stored ancestor; the synthesis of page k goes to `out` page k. */
    syntheses: { page: Page; stored: number }[];
    /** The stamps of the syntheses, then those of stored pages, whose heights go to `out` after the syntheses (`copies`). */
    stamps: Stamp[];
    /** The pages that draw new heights. */
    fresh: Page[];
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
 * ago leaves first, and a frame that wants more pages than a pool holds counts a shortage. The mask
 * layers go to the nearest near pages with features.
 */
export class Pages {
    requests = 0;
    bytes = 0;
    failed = 0;
    /** Wanted pages that found no free slot in the last frame. */
    shortage = 0;
    /** Wanted pages that are not ready yet in the last frame. */
    missing = 0;
    /** Of them, the pages of `near` and of `eye` in the last update. */
    nearMissing = 0;
    eyeMissing = 0;
    private near = new Set<string>();
    private eye = new Set<string>();
    /** Pages of each pool that the last frame wanted. */
    wanted: Record<Kind, number> = { stored: 0, synth: 0 };
    readonly pages = new Map<string, Page>();
    private readonly tiles = new Map<string, Tile>();
    private readonly bodies = new Map<string, ArrayBuffer>();
    /** The requests for bodies in flight. */
    private readonly requested = new Map<string, Promise<ArrayBuffer | undefined>>();
    private bodyBytes = 0;
    private readonly free: Record<Kind, number[]>;
    private maskFree: number[];
    /** Metres from the eye to the nearest near page with features that the mask pool cannot hold: the paint fades out before it. */
    maskReach = NO_LIMIT;
    private arrivals: { node: Node; data: PageData }[] = [];
    /** Tiles whose decoded pages wait in `arrivals`, by name. */
    private readonly queued = new Map<string, number>();
    /** The tiles that the frame's pages wait for, by name, with those pages: the frame starts the most needed of them. */
    private readonly wishes = new Map<string, { tile: Node; pages: Map<string, Node> }>();
    /** The pages that the frame draws or synthesizes from, by name: an arrival among them evicts none of them. */
    private readonly needs = new Set<string>();
    private frame = 0;
    /** Tile loads and decodes in flight. */
    loads = 0;
    private dirty = true;
    /** The masks changed, or the reach of the paint, while the LOD map did not. */
    private masksDirty = true;
    private origins: [number, number][] = [];

    constructor(private readonly source: TerrainSource, readonly capacity: Record<Kind, number>, private readonly bounds: [number, number, number, number], private readonly changed: () => void,
        private readonly stamps: StampSource = NO_STAMPS, readonly maskCapacity = 0) {
        this.free = { stored: range(0, capacity.stored), synth: range(capacity.stored, capacity.stored + capacity.synth) };
        this.maskFree = range(0, maskCapacity);
    }

    get idle(): boolean {
        return this.missing === 0 && this.loads === 0 && this.arrivals.length === 0;
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

    /**
     * Takes the pages that a frame wants, coarse and near first, and returns its GPU work. `near` and
     * `eye` name the wanted pages that `nearMissing` and `eyeMissing` count; those of the eye field ask
     * for their features and synthesize first.
     */
    update(wanted: Node[], anchor: Anchor, near = new Set<string>(), eye = new Set<string>()): Work {
        this.frame++;
        this.near = near;
        this.eye = eye;
        this.shortage = 0;
        this.missing = this.nearMissing = this.eyeMissing = 0;
        this.wanted = { stored: 0, synth: 0 };
        const work: Work = { uploads: [], syntheses: [], stamps: [], fresh: [], origins: this.window(anchor) };
        // A node that nothing draws yet also wants its page of level FILL, which is a hole until it comes.
        const fills = new Map<string, Node>();
        for (const node of wanted) {
            if (node.level <= FILL || this.pages.get(key(node))?.drawn !== undefined || this.covers(node)) continue;
            const up = node.level - FILL, fill = { level: FILL, x: node.x >> up, y: node.y >> up };
            fills.set(key(fill), fill);
        }
        wanted = [...fills.values(), ...wanted.filter(node => eye.has(key(node))), ...wanted.filter(node => !eye.has(key(node)))];
        // Every wanted page is used before the first allocation, so none leaves for a later one of this frame.
        for (const node of wanted) {
            const page = this.pages.get(key(node));
            if (page) page.used = this.frame;
        }
        const holders = this.holders(wanted, anchor);
        this.wishes.clear();
        this.needs.clear();
        for (const node of wanted) this.want(node, work, holders);
        this.start(anchor);
        while (work.uploads.length + work.syntheses.length < ARRIVALS && this.arrivals.length) {
            const { node, data } = this.arrivals.shift()!;
            const tile = `${node.level - 1}/${node.x >> 1}/${node.y >> 1}`, heights: [number, number] = [data.min - summit(node.level), data.max + summit(node.level)];
            this.queued.set(tile, this.queued.get(tile)! - 1);
            const page = this.pages.get(key(node)) ?? this.allocate(node, 'stored', heights, this.needs.has(key(node)));
            if (!page || page.ready) continue;
            page.range = heights;
            page.complete = data.complete;
            page.ready = true;
            work.uploads.push({ page, data });
        }
        // A stored page draws once its stamps are on: as it is, or as a stamped copy.
        const fresh = work.fresh;
        fresh.push(...work.syntheses.map(({ page }) => page));
        for (const page of fresh) page.ready = true;
        for (const page of this.pages.values()) {
            if (page.kind === 'stored' && page.ready && page.drawn === undefined && page.used === this.frame && this.resolve(page, work)) fresh.push(page);
        }
        // The masks of drawn pages wait for the copies, which a stored page needs to draw at all.
        this.lateMasks(holders, work);
        if (fresh.length) this.dirty = true;
        if (this.dirty) work.map = this.map();
        if (this.dirty || this.masksDirty) work.masks = this.masks();
        this.dirty = this.masksDirty = false;
        return work;
    }

    /** A lost device keeps the tiles that were absent and their bodies; every page comes again. */
    lost(): void {
        this.pages.clear();
        this.maskFree = range(0, this.maskCapacity);
        this.arrivals = [];
        this.queued.clear();
        this.free.stored = range(0, this.capacity.stored);
        this.free.synth = range(this.capacity.stored, this.capacity.stored + this.capacity.synth);
        for (const tile of this.tiles.values()) if (tile.state === 'decoding') tile.state = 'done';
        this.dirty = true;
    }

    private want(node: Node, work: Work, holders: Map<string, Node>): void {
        const page = this.pages.get(key(node));
        if (page) {
            page.used = this.frame;
            this.wanted[page.kind]++;
            if (page.drawn === undefined) {
                this.miss(node);
                // A ready stored page asks for its features in the frame's order: `resolve` puts them on.
                this.stamps.get(node);
            }
            return;
        }
        const stored = this.stored(node);
        if (stored !== 'no') {
            this.wanted.stored++;
            this.miss(node);
            this.load(node);
            return;
        }
        const ancestor = this.storedAncestor(node);
        // No stored tile above the node knows a height: nothing draws there.
        if (!ancestor) return;
        this.wanted.synth++;
        this.miss(node);
        if (ancestor === 'wait') return;
        // The features load while the inputs do.
        const stamps = this.stamps.get(node);
        // The synthesis reads the ancestor and its neighbours: no allocation of this frame takes them.
        const inputs = this.around(ancestor).map(near => {
            const page = this.pages.get(key(near));
            if (page) page.used = this.frame;
            return { near, page };
        });
        const missing = inputs.filter(({ near, page }) => !page?.ready && this.stored(near) !== 'no');
        for (const { near, page } of missing) if (!page) this.load(near);
        if (missing.length || !stamps || this.busy(work, stamps.data?.length)) return;
        const synth = this.allocate(node, 'synth', this.range(ancestor)!, true);
        if (!synth) return;
        synth.drawn = synth.slot;
        if (stamps.data) {
            const mode = (stamps.heights ? MODE.HEIGHTS : 0) | (holders.has(key(node)) && this.mask(synth, holders, work) ? MODE.MASK : 0);
            if (mode) work.stamps.push({ page: synth, out: work.syntheses.length, target: synth.slot, data: stamps.data, mode });
        }
        work.syntheses.push({ page: synth, stored: ancestor.level });
    }

    /** Counts a wanted page that does not draw yet. */
    private miss(node: Node): void {
        this.missing++;
        if (this.near.has(key(node))) this.nearMissing++;
        if (this.eye.has(key(node))) this.eyeMissing++;
    }

    /**
     * The near pages with known features that hold the masks in this frame, nearest first, by name: as
     * many as the pool holds. The first page beyond them sets the reach of the paint. It asks for no
     * features: a page asks for its own once its stored ancestor is in, the eye field first, so the
     * page under the eye does not wait behind the near pages.
     */
    private holders(wanted: Node[], anchor: Anchor): Map<string, Node> {
        const near = wanted.filter(node => node.level >= NEAR && this.stamps.known(node)?.mask)
            .map(node => ({ node, distance: distances({ anchor, above: 0 }, node)[0] })).sort((a, b) => a.distance - b.distance);
        const reach = near.length > this.maskCapacity ? near[this.maskCapacity].distance : NO_LIMIT;
        if (reach !== this.maskReach) this.masksDirty = true;
        this.maskReach = reach;
        return new Map(near.slice(0, this.maskCapacity).map(({ node }) => [key(node), node]));
    }

    /** A mask-only job for each holder without a mask, nearest first, as the frame has room: a page synthesized while the pool was full gets its mask later. */
    private lateMasks(holders: Map<string, Node>, work: Work): void {
        for (const node of holders.values()) {
            const page = this.pages.get(key(node)), data = this.stamps.get(node)?.data;
            if (!page || !data || page.kind !== 'synth' || page.drawn === undefined || page.mask !== undefined) continue;
            if (this.busy(work, data.length) || !this.mask(page, holders, work)) return;
            work.stamps.push({ page, out: 0, target: page.slot, data, mode: MODE.MASK });
        }
    }

    /** Whether the frame has no room for another arrival, synthesis, copy or stamp job, or for `words` of stamp data. */
    private busy(work: Work, words = 0): boolean {
        const used = work.stamps.reduce((sum, { data }) => sum + (data?.length ?? 0), 0);
        return work.uploads.length + work.syntheses.length + copies(work).length >= ARRIVALS || work.stamps.length >= ARRIVALS || used + words > STAMP_WORDS;
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
     * A mask layer for a holder: a free one, or that of the page wanted longest ago that is no holder
     * and has no stamp job in this frame, which would write the layer.
     */
    private mask(page: Page, holders: Map<string, Node>, work: Work): boolean {
        let slot = this.maskFree.pop();
        if (slot === undefined) {
            let donor: Page | undefined;
            for (const other of this.pages.values()) {
                if (other.mask === undefined || holders.has(key(other.node)) || work.stamps.some(stamp => stamp.page === other)) continue;
                if (!donor || other.used < donor.used) donor = other;
            }
            if (!donor) return false;
            slot = donor.mask;
            donor.mask = undefined;
        }
        page.mask = slot;
        this.masksDirty = true;
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
            this.load(parent);
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

    /** A stored page that the frame needs: the frame loads its tile if it is among the most needed. */
    private load(node: Node): void {
        this.needs.add(key(node));
        const tile: Node = { level: node.level - 1, x: node.x >> 1, y: node.y >> 1 }, name = key(tile), state = this.tiles.get(name)?.state;
        if ((state && state !== 'done') || this.queued.get(name)) return;
        const wish = this.wishes.get(name) ?? { tile, pages: new Map<string, Node>() };
        wish.pages.set(key(node), node);
        this.wishes.set(name, wish);
    }

    /** Starts the wished tiles in the order of `priority`, of the most valuable page that each holds; the finer tile first at a tie. */
    private start(anchor: Anchor): void {
        if (this.loads >= LOADS) return;
        const eye = { anchor, above: 0 };
        const order = [...this.wishes.values()].map(({ tile, pages }) => ({
            tile, value: Math.max(...[...pages.values()].map(page => priority(page.level, this.coming(page), distances(eye, page)[0]))),
        })).sort((a, b) => b.value - a.value || b.tile.level - a.tile.level);
        for (const { tile } of order.slice(0, LOADS - this.loads)) this.fetch(tile.level, tile.x, tile.y);
    }

    /**
     * The level of the page that draws first in a node's place: the finest above it that exists or
     * whose tile is on its way, else the coarsest whose tile the frame wishes, which goes first as a
     * hole. Undefined where nothing comes: a hole.
     */
    private coming(node: Node): number | undefined {
        let wished: number | undefined;
        for (let { level, x, y } = node; level > COARSEST;) {
            level--; x >>= 1; y >>= 1;
            const tile = `${level - 1}/${x >> 1}/${y >> 1}`, state = this.tiles.get(tile)?.state;
            if (this.pages.has(`${level}/${x}/${y}`) || state === 'loading' || state === 'decoding' || this.queued.get(tile)) return level;
            if (this.wishes.has(tile)) wished = level;
        }
        return wished;
    }

    /** The body of a stored tile for the roads, undefined where it is absent: a request in flight serves them too. */
    body(z: number, x: number, y: number): Promise<ArrayBuffer | undefined> {
        const name = `${z}/${x}/${y}`, kept = this.bodies.get(name);
        if (kept || this.tiles.get(name)?.state === 'absent') return Promise.resolve(kept);
        const request = this.requested.get(name) ?? this.request(z, x, y);
        return request.then(data => this.arrived(name, data));
    }

    private request(z: number, x: number, y: number): Promise<ArrayBuffer | undefined> {
        const name = `${z}/${x}/${y}`, request = this.source.fetch(z, x, y);
        this.requests++;
        this.requested.set(name, request);
        return request;
    }

    /** Keeps the body of a request that ended, once; its absence stays known. */
    private arrived(name: string, data: ArrayBuffer | undefined): ArrayBuffer | undefined {
        if (!this.requested.delete(name)) return data;
        if (data) {
            this.bytes += data.byteLength;
            this.keep(name, data);
        } else this.tiles.set(name, { state: 'absent' });
        return data;
    }

    /** Requests a stored tile, or decodes its kept body again. */
    private fetch(z: number, x: number, y: number): void {
        const name = `${z}/${x}/${y}`, tile = this.tiles.get(name);
        const body = this.bodies.get(name);
        this.loads++;
        this.tiles.set(name, { state: body ? 'decoding' : 'loading', empty: tile?.empty });
        (async () => {
            let data = body;
            if (!data) {
                data = this.arrived(name, await (this.requested.get(name) ?? this.request(z, x, y)));
                if (!data) return this.tiles.set(name, { state: 'absent' });
            }
            const quadrants = await this.source.decode(data);
            this.tiles.set(name, { state: 'done', empty: quadrants.map(quadrant => !quadrant) });
            quadrants.forEach((data, q) => {
                if (data) this.arrivals.push({ node: { level: z + 1, x: 2 * x + (q & 1), y: 2 * y + (q >> 1) }, data });
            });
            this.queued.set(name, quadrants.filter(Boolean).length);
        })().catch(error => {
            // A tile that fails stays out for this page, as an absent one.
            if (!stopped(error)) console.warn(`Terrain tile ${name}: ${error instanceof Error ? error.message : error}`);
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
     * frame wants them all: a shortage when the frame needs the new page too.
     */
    private allocate(node: Node, kind: Kind, heights: [number, number], needed: boolean): Page | undefined {
        const slot = this.slot(kind);
        if (slot === undefined) {
            if (needed) this.shortage++;
            return undefined;
        }
        // A synthesized page reads its stored ancestors only, and `heights` holds all that they draw.
        // A page that the frame does not need counts as used a frame ago: a needed arrival of the frame can take its slot.
        const page: Page = { node, kind, slot, used: needed ? this.frame : this.frame - 1, range: heights, complete: true, ready: false };
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

    /** As `Masks` in features.wgsl: the reach of the paint, a word that terrain.ts writes, then per layer the mask layer + 1, 0 for none. */
    private masks(): Uint32Array<ArrayBuffer> {
        const masks = new Uint32Array(2 + this.capacity.stored + this.capacity.synth);
        new Float32Array(masks.buffer, 0, 1)[0] = this.maskReach;
        for (const page of this.pages.values()) if (page.drawn !== undefined && page.mask !== undefined) masks[2 + page.drawn] = page.mask + 1;
        return masks;
    }
}

/** A reach that no view meets: no shader takes an infinity. */
export const NO_LIMIT = 1e9;

/** The bands of `priority` above the error over distance, which is at most size(COARSEST) over a metre. */
const NEAR_FIELD = 2 ** 30, HOLE = 2 ** 40;

/**
 * The value of a stored page of `level` at `distance` metres from the eye, where the page of level
 * `coming` draws or is on its way: the one rule of the load order.
 * - A hole (nothing comes) goes first, the coarsest first: one coarse tile fills much of the view,
 *   and then every finer page has a page above it to draw from.
 * - Then the pages of the deepest stored level, which the near field draws and synthesizes from.
 * - In a band, the value is the screen error that the page takes away: the size of the page in its
 *   place minus its own, over the distance. So the page under the eye comes first of the near field,
 *   and the coarse cover far away comes last.
 */
function priority(level: number, coming: number | undefined, distance: number): number {
    if (coming === undefined) return HOLE - level;
    return (level > TILE_ZOOM ? NEAR_FIELD : 0) + (size(coming) - size(level)) / Math.max(distance, 1);
}

const range = (from: number, to: number) => Array.from({ length: to - from }, (_, i) => to - 1 - i);
