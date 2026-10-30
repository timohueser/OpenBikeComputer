import { featureTile, type NodeStamps } from './bins';
import { key, type Node } from './lod';
import { STAMP_WORDS, type StampSource } from './pages';
import type { Request } from './stamp-worker';
import type { NodeMeshes } from '../structures/node';
import type { WaterMesh } from './water-mesh';
import { stopped } from '../stream/terrain';

/** The bytes of the binned nodes and their water meshes kept; an entry costs ENTRY_BYTES more. */
// PROTOTYPE (road study): room for the corridor meshes and their roadside details.
const NODE_BYTES = 192 * 2 ** 20, ENTRY_BYTES = 64;
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
    private readonly worker?: Worker;
    private readonly waiting = new Map<number, { resolve: (result: unknown) => void; reject: (error: Error) => void }>();
    private next = 0;
    failed = 0;

    /** An abort of `stop` ends the worker. */
    constructor(private readonly tiles: FeatureTiles | undefined, private readonly changed: () => void, stop: AbortSignal) {
        if (!tiles) return;
        const worker = this.worker = new Worker(new URL('./stamp-worker.ts', import.meta.url), { type: 'module' });
        stop.addEventListener('abort', () => worker.terminate());
        this.worker.onmessage = ({ data }) => {
            const wait = this.waiting.get(data.id);
            this.waiting.delete(data.id);
            if (data.error) wait?.reject(new Error(data.error));
            else wait?.resolve(data.result);
        };
    }

    /** Whether features are on their way: each wakes the engine when it arrives. */
    get loading(): boolean {
        return this.pending.size > 0;
    }

    /** The node's stamps and water, undefined while they are on their way. */
    get(node: Node): NodeFeatures | undefined {
        const tile = featureTile(node);
        if (!this.tiles || !tile) return NOTHING;
        const known = this.known(node);
        if (known) return known;
        const name = key(node);
        if (!this.pending.has(name) && this.pending.size < REQUESTS) {
            this.pending.add(name);
            this.ask<NodeFeatures>(tile, node, 'stamps').then(stamps => {
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
        return undefined;
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

    private async ask<T>(tile: Node, node: Node, what: Request): Promise<T> {
        const body = await this.tiles!(tile.level, tile.x, tile.y);
        if (!body) return ({ stamps: NOTHING, samples: NO_SAMPLES } as const)[what] as T;
        const id = this.next++;
        return new Promise<T>((resolve, reject) => {
            this.waiting.set(id, { resolve: resolve as (result: unknown) => void, reject });
            this.worker!.postMessage({ id, tile, node, body, what }, [body]);
        });
    }
}
