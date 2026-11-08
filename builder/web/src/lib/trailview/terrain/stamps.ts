import { featureTile, type NodeStamps } from './bins';
import { CAPS, DeviceCache } from './device-cache';
import { key, OWNER, type Node } from './lod';
import { BAKED, decodeNode, EMPTY_MESHES, encodeNode, unpackNode } from './node-bytes';
import { STAMP_WORDS, type StampSource } from './pages';
import type { Request } from './stamp-worker';
import type { NodeMeshes } from '../structures/node';
import type { WaterMesh } from './water-mesh';
import { stopped } from '../stream/terrain';

/** The bytes of the binned nodes and their water meshes kept; an entry costs ENTRY_BYTES more. */
const NODE_BYTES = 64 * 2 ** 20, ENTRY_BYTES = 64;
/** Requests to the worker in flight at most. */
const REQUESTS = 8;

/** What a node stamps, its water, its structures and kinds, their floors and the boxes of its tunnels' portals, from the worker. */
export interface NodeFeatures extends NodeStamps {
    water: WaterMesh | null;
    meshes: NodeMeshes | null;
    floors: Float64Array<ArrayBuffer> | null;
    portals: Float64Array<ArrayBuffer> | null;
}

const NOTHING: NodeFeatures = { data: null, heights: false, mask: false, water: null, meshes: null, floors: null, portals: null };
const meshBytes = (mesh: { vertices: ArrayBuffer; indices: Uint32Array } | null) => (mesh ? mesh.vertices.byteLength + mesh.indices.byteLength : 0);
const kindBytes = ({ far, near, anchors, points }: NodeMeshes) =>
    [far.structures, far.anchored, near.structures, near.anchored].reduce((n, mesh) => n + meshBytes(mesh), (anchors?.byteLength ?? 0) + (points?.byteLength ?? 0));
const bytesOf = ({ data, water, meshes, floors, portals }: NodeFeatures) =>
    ENTRY_BYTES + (data?.byteLength ?? 0) + meshBytes(water) + (meshes ? kindBytes(meshes) : 0) + (floors?.byteLength ?? 0) + (portals?.byteLength ?? 0);

/** The body of feature tile z/x/y, undefined where it is absent. */
export type FeatureTiles = (z: number, x: number, y: number) => Promise<ArrayBuffer | undefined>;
/** The bytes of node z/x/y of the stamp layer (node-bytes.ts), undefined where the layer holds none. */
export type BakedNodes = (z: number, x: number, y: number) => Promise<Uint8Array | undefined>;

/** The audit samples of a node, as audit.wgsl reads them, and the water levels of its feature tile. */
export interface NodeSamples {
    samples: Uint32Array<ArrayBuffer>;
    lakes: number;
    mixed: number;
    rivers: number;
    rising: number;
}

const NO_SAMPLES: NodeSamples = { samples: new Uint32Array(0), lakes: 0, mixed: 0, rivers: 0, rising: 0 };

/**
 * The binned features and the water mesh of each node, made in a worker. A node without a feature tile
 * above it, or without a feature archive, stamps nothing and has no water.
 */
export class Stamps implements StampSource {
    private readonly nodes = new Map<string, NodeFeatures>();
    private nodeBytes = 0;
    private readonly pending = new Set<string>();
    private readonly masks = new Map<string, Promise<number | undefined>>();
    /** The nodes that the frame asked for and that are not on their way. */
    private readonly wishes = new Map<string, Node>();
    private readonly worker?: Worker;
    private readonly waiting = new Map<number, { resolve: (result: unknown) => void; reject: (error: Error) => void }>();
    private next = 0;
    private readonly cache?: Promise<DeviceCache | undefined>;
    failed = 0;

    /**
     * An abort of `stop` ends the worker. With `plan` the records draw the ways (nodeStamps of bins.ts). A node
     * that `baked` holds comes from it, with the same bytes as from the worker. With `cache`, the release and the
     * version of the stamp code (`virtual:stamp-version`), the nodes go to the device cache.
     */
    constructor(private readonly tiles: FeatureTiles | undefined, private readonly changed: () => void, stop: AbortSignal, private readonly plan = false, private readonly baked?: BakedNodes, cache?: { release: string; version: string }) {
        if (!tiles) return;
        if (cache) this.cache = DeviceCache.open('stamps', cache.release, `${cache.version}/${plan}`, CAPS.stamps).catch(error => {
            console.warn(`The stamps' device cache did not open: ${error}`);
            return undefined;
        });
        const worker = this.worker = new Worker(new URL('./stamp-worker.ts', import.meta.url), { type: 'module' });
        stop.addEventListener('abort', () => worker.terminate());
        this.worker.onmessage = ({ data }) => {
            const wait = this.waiting.get(data.id);
            this.waiting.delete(data.id);
            if (data.error) wait?.reject(new Error(data.error));
            else wait?.resolve(data.result);
        };
    }

    /** Whether features are on their way: each wakes the engine when it arrives. A wish waits only while REQUESTS are. */
    get loading(): boolean {
        return this.pending.size > 0;
    }

    /** The node's stamps and water, undefined while they are on their way: the node is a wish of this frame until `send`. */
    get(node: Node): NodeFeatures | undefined {
        const tile = featureTile(node);
        if (!this.tiles || !tile) return NOTHING;
        const known = this.known(node);
        if (known) return known;
        const name = key(node);
        if (!this.pending.has(name)) this.wishes.set(name, node);
        return undefined;
    }

    /**
     * Asks the worker for the frame's wishes, the lowest `rank` first, while it has fewer than REQUESTS
     * on their way. The rest wait for the next frame, which states its wishes again.
     */
    send(rank: (node: Node) => number): void {
        const free = REQUESTS - this.pending.size, wishes = [...this.wishes.values()];
        this.wishes.clear();
        if (free <= 0) return;
        const ranks = new Map(wishes.map(node => [node, rank(node)]));
        for (const node of wishes.sort((a, b) => ranks.get(a)! - ranks.get(b)!).slice(0, free)) this.request(node);
    }

    private request(node: Node): void {
        const tile = featureTile(node)!, name = key(node);
        this.pending.add(name);
        this.stamps(tile, node).then(stamps => {
            // A frame takes at most STAMP_WORDS: a node with more would wait forever, so it stamps nothing.
            if ((stamps.data?.length ?? 0) <= STAMP_WORDS) return this.keep(name, stamps);
            console.warn(`Node ${name}: ${stamps.data!.length} words of stamps, more than a frame takes`);
            this.keep(name, { ...stamps, data: null, heights: false, mask: false });
        }, error => {
            // A tile that fails stamps nothing, as an absent one.
            if (!stopped(error)) console.warn(`Feature tile ${key(tile)}: ${error instanceof Error ? error.message : error}`);
            this.failed++;
            this.keep(name, NOTHING);
        }).finally(() => {
            this.pending.delete(name);
            this.changed();
        });
    }

    /** A node's stamps from the device cache, or from the worker, which the cache then keeps. */
    private async stamps(tile: Node, node: Node): Promise<NodeFeatures> {
        const cache = await this.cache, name = key(node);
        const hit = await cache?.read(name).catch(() => undefined);
        if (hit) return decodeNode(new Uint8Array(hit[0].buffer)) as NodeFeatures;
        const made = await this.features(tile, node);
        // Off the critical path; a value that encodeNode cannot give back exactly is not kept.
        if (cache && made !== NOTHING) void Promise.resolve().then(() => cache.write(name, [new Uint32Array(encodeNode(made).buffer)])).catch(() => {});
        return made;
    }

    /**
     * A BAKED node with the ground plan, and an owner node's structure meshes, from the stamp layer where it holds
     * them; the rest from the worker. The mask of a feature tile says which of its owner nodes have meshes: the
     * others are EMPTY_MESHES and cost no request. Each node that the layer cannot give comes from the worker whole.
     */
    private async features(tile: Node, node: Node): Promise<NodeFeatures> {
        if (this.plan && node.level >= BAKED.from && node.level <= BAKED.to) {
            const whole = await this.read(node);
            if (whole) return whole as NodeFeatures;
        } else if (node.level === OWNER && this.baked) {
            const mask = await this.mask(tile);
            if (mask !== undefined) {
                const held = (mask >> ((node.x & 3) + 4 * (node.y & 3))) & 1;
                const [stamps, meshes] = await Promise.all([this.ask<NodeFeatures>(tile, node, 'stamps', false), held ? this.read(node) : EMPTY_MESHES]);
                if (stamps === NOTHING) return NOTHING;
                if (meshes) return { ...stamps, meshes: meshes as NodeMeshes };
            }
        }
        return this.ask<NodeFeatures>(tile, node, 'stamps');
    }

    /** The node as the stamp layer holds it; undefined where it holds none or its bytes do not decode. */
    private async read(node: Node): Promise<unknown> {
        try {
            const bytes = await this.baked?.(node.level, node.x, node.y);
            return bytes && (await unpackNode(bytes));
        } catch {
            return undefined;
        }
    }

    /** The mask of the owner nodes of feature tile `tile` that the stamp layer holds; undefined without one. */
    private mask(tile: Node): Promise<number | undefined> {
        const name = key(tile);
        let mask = this.masks.get(name);
        if (!mask) {
            const read = this.baked!(tile.level, tile.x, tile.y).then(bytes => (bytes?.byteLength === 2 ? bytes[0] | (bytes[1] << 8) : undefined));
            this.masks.set(name, (mask = read.catch(() => undefined)));
        }
        return mask;
    }

    /** The audit samples of a node. */
    async samples(node: Node): Promise<NodeSamples> {
        const tile = featureTile(node);
        return this.tiles && tile ? this.ask<NodeSamples>(tile, node, 'samples') : NO_SAMPLES;
    }

    /** The node's stamps and water once they are known, without a request. */
    known(node: Node): NodeFeatures | undefined {
        if (!this.tiles || !featureTile(node)) return NOTHING;
        const name = key(node), known = this.nodes.get(name);
        if (known) {
            this.nodes.delete(name);
            this.nodes.set(name, known);
        }
        return known;
    }

    private keep(name: string, stamps: NodeFeatures): void {
        const old = this.nodes.get(name);
        if (old) this.nodeBytes -= bytesOf(old);
        this.nodes.set(name, stamps);
        this.nodeBytes += bytesOf(stamps);
        for (const [first, kept] of this.nodes) {
            if (this.nodeBytes <= NODE_BYTES) break;
            this.nodes.delete(first);
            this.nodeBytes -= bytesOf(kept);
        }
    }

    /** Without `meshes` the worker leaves the structure meshes out. */
    private async ask<T>(tile: Node, node: Node, what: Request, meshes = true): Promise<T> {
        const body = await this.tiles!(tile.level, tile.x, tile.y);
        if (!body) return ({ stamps: NOTHING, samples: NO_SAMPLES } as const)[what] as T;
        const id = this.next++;
        return new Promise<T>((resolve, reject) => {
            this.waiting.set(id, { resolve: resolve as (result: unknown) => void, reject });
            this.worker!.postMessage({ id, tile, node, body, what, plan: this.plan, meshes }, [body]);
        });
    }
}
