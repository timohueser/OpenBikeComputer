import type { Anchor } from '../frame/space';
import type { PageData } from '../stream/heights';
import { TILE_ZOOM, type TerrainSource } from '../stream/terrain';
import { COARSEST, FINEST, key, MAP, size, type Node } from './lod';

/** Tile requests in flight at most. */
const LOADS = 6;
/** Pages that arrive or are synthesized per frame at most; each gets its normals in the same frame. */
export const ARRIVALS = 16;
/** Pages whose normals are computed again per frame at most, after a neighbour arrived. */
const REFRESHES = 8;
/** Encoded tile bodies kept, so a new device gets the pages again without a request. */
const BODIES = 64 * 2 ** 20;
/** A LOD map entry: the page layer + 1, and this bit for a stored page. */
export const STORED_BIT = 0x80000000;

export type Kind = 'stored' | 'synth';

export interface Page {
    node: Node;
    kind: Kind;
    /** The layer in the page arrays. */
    slot: number;
    /** The last frame that wanted the page. */
    used: number;
    /** Heights for culling: a synthesized page has its stored ancestor's. */
    range: [number, number];
    /** The heights and normals are on the GPU. */
    ready: boolean;
}

/** The GPU work of one frame, in order: uploads and syntheses, then the normals, then the LOD map. */
export interface Work {
    uploads: { page: Page; data: PageData }[];
    /** Each with the level of its stored ancestor. */
    syntheses: { page: Page; stored: number }[];
    normals: Page[];
    /** The LOD map: MAP × MAP entries per level from 0 to FINEST, when it changed. */
    map?: Uint32Array<ArrayBuffer>;
    /** The node of each level at the LOD map window's origin. */
    origins: [number, number][];
}

interface Tile {
    state: 'loading' | 'decoding' | 'done' | 'absent';
    /** Quadrants without a known height, once decoded. */
    empty?: boolean[];
}

/**
 * The page pools and what fills them. Stored pages come from the stored tiles, synthesized pages from
 * their deepest stored ancestor. The pools have a fixed size: the page that the frames wanted longest
 * ago leaves first, and a frame that wants more pages than a pool holds counts a shortage.
 */
export class Pages {
    requests = 0;
    bytes = 0;
    failed = 0;
    /** Wanted pages that found no free slot in the last frame. */
    shortage = 0;
    /** Wanted pages that are not ready yet in the last frame. */
    missing = 0;
    readonly pages = new Map<string, Page>();
    private readonly tiles = new Map<string, Tile>();
    private readonly bodies = new Map<string, ArrayBuffer>();
    private bodyBytes = 0;
    private readonly free: Record<Kind, number[]>;
    private arrivals: { node: Node; data: PageData }[] = [];
    private readonly refresh = new Set<Page>();
    private frame = 0;
    /** Tile loads and decodes in flight. */
    loads = 0;
    private dirty = true;
    private origins: [number, number][] = [];

    constructor(private readonly source: TerrainSource, readonly capacity: Record<Kind, number>, private readonly bounds: [number, number, number, number], private readonly changed: () => void) {
        this.free = { stored: range(0, capacity.stored), synth: range(capacity.stored, capacity.stored + capacity.synth) };
    }

    get idle(): boolean {
        return this.missing === 0 && this.loads === 0 && this.arrivals.length === 0 && this.refresh.size === 0;
    }

    /** The heights of the finest page that covers a node, or of none. */
    range(node: Node): [number, number] | undefined {
        for (let { level, x, y } = node; level >= COARSEST; level--, x >>= 1, y >>= 1) {
            const page = this.pages.get(`${level}/${x}/${y}`);
            if (page) return page.range;
        }
        return undefined;
    }

    /** Takes the pages that a frame wants, coarse and near first, and returns its GPU work. */
    update(wanted: Node[], anchor: Anchor): Work {
        this.frame++;
        this.shortage = 0;
        this.missing = 0;
        const work: Work = { uploads: [], syntheses: [], normals: [], origins: this.window(anchor) };
        for (const node of wanted) this.want(node, work);
        const keys = new Set(wanted.map(key));
        while (work.uploads.length + work.syntheses.length < ARRIVALS && this.arrivals.length) {
            const { node, data } = this.arrivals.shift()!;
            const page = this.pages.get(key(node)) ?? this.allocate(node, 'stored', [data.min, data.max], keys.has(key(node)));
            if (!page || page.ready) continue;
            page.range = [data.min, data.max];
            work.uploads.push({ page, data });
        }
        for (const { page } of [...work.uploads, ...work.syntheses]) {
            page.ready = true;
            this.dirty = true;
            work.normals.push(page);
            this.refresh.delete(page);
            for (const neighbour of this.neighbours(page.node)) if (neighbour.ready) this.refresh.add(neighbour);
        }
        for (const page of [...this.refresh].slice(0, REFRESHES)) {
            this.refresh.delete(page);
            if (page.ready && !work.normals.includes(page)) work.normals.push(page);
        }
        if (this.dirty) work.map = this.map();
        this.dirty = false;
        return work;
    }

    /** A lost device keeps the tiles that were absent and their bodies; every page comes again. */
    lost(): void {
        this.pages.clear();
        this.arrivals = [];
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
            if (!page.ready) this.missing++;
            return;
        }
        const stored = this.stored(node);
        if (stored !== 'no') {
            this.missing++;
            this.load(node.level - 1, node.x >> 1, node.y >> 1);
            return;
        }
        const ancestor = this.storedAncestor(node);
        // No stored tile above the node knows a height: nothing draws there.
        if (!ancestor) return;
        this.missing++;
        if (ancestor === 'wait' || work.uploads.length + work.syntheses.length >= ARRIVALS) return;
        const ready = this.around(ancestor).every(near => {
            const state = this.stored(near);
            if (state === 'no' || this.pages.get(key(near))?.ready) return true;
            this.load(near.level - 1, near.x >> 1, near.y >> 1);
            return false;
        });
        if (!ready) return;
        const synth = this.allocate(node, 'synth', this.pages.get(key(ancestor))!.range, true);
        if (synth) work.syntheses.push({ page: synth, stored: ancestor.level });
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
            if (state === 'yes' && this.pages.get(key(parent))?.ready) return parent;
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
        if (tile && tile.state !== 'done') return;
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
        let slot = this.free[kind].pop();
        if (slot === undefined) {
            let oldest: Page | undefined;
            for (const page of this.pages.values()) if (page.kind === kind && page.used < this.frame && (!oldest || page.used < oldest.used)) oldest = page;
            if (!oldest) {
                if (wanted) this.shortage++;
                return undefined;
            }
            this.pages.delete(key(oldest.node));
            this.refresh.delete(oldest);
            if (oldest.ready) this.dirty = true;
            slot = oldest.slot;
        }
        const page: Page = { node, kind, slot, used: this.frame, range: heights, ready: false };
        this.pages.set(key(node), page);
        return page;
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

    private map(): Uint32Array<ArrayBuffer> {
        const map = new Uint32Array((FINEST + 1) * MAP * MAP);
        for (const page of this.pages.values()) {
            if (!page.ready) continue;
            const { level, x, y } = page.node, [ox, oy] = this.origins[level], i = x - ox, j = y - oy;
            if (i < 0 || j < 0 || i >= MAP || j >= MAP) continue;
            map[(level * MAP + j) * MAP + i] = (page.slot + 1) | (page.kind === 'stored' ? STORED_BIT : 0);
        }
        return map;
    }
}

const range = (from: number, to: number) => Array.from({ length: to - from }, (_, i) => to - 1 - i);
