import { featureTile, type NodeStamps } from './bins';
import { key, type Node } from './lod';
import type { StampSource } from './pages';
import type { Request } from './stamp-worker';
import type { StructureMesh } from '../structures/mesh';
import type { WaterMesh } from './water-mesh';

/** Feature tile bodies kept, and the bytes of the binned nodes and their water meshes kept; an entry costs ENTRY_BYTES more. */
const BODIES = 32 * 2 ** 20, NODE_BYTES = 64 * 2 ** 20, ENTRY_BYTES = 64;
/** Requests to the worker in flight at most. */
const REQUESTS = 8;

/** What a node stamps, its water and its structures, from the worker. */
export interface NodeFeatures extends NodeStamps {
    water: WaterMesh | null;
    structures: StructureMesh | null;
}

const NOTHING: NodeFeatures = { data: null, heights: false, water: null, structures: null };
const meshBytes = (mesh: WaterMesh | StructureMesh | null) => (mesh ? mesh.vertices.byteLength + mesh.indices.byteLength : 0);
const bytesOf = ({ data, water, structures }: NodeFeatures) => ENTRY_BYTES + (data?.byteLength ?? 0) + meshBytes(water) + meshBytes(structures);

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
    private readonly bodies = new Map<string, Promise<ArrayBuffer | undefined>>();
    private readonly sizes = new Map<string, number>();
    private bodyBytes = 0;
    private readonly worker?: Worker;
    private readonly waiting = new Map<number, { resolve: (result: unknown) => void; reject: (error: Error) => void }>();
    private next = 0;
    failed = 0;

    constructor(private readonly tiles: FeatureTiles | undefined, private readonly changed: () => void) {
        if (!tiles) return;
        this.worker = new Worker(new URL('./stamp-worker.ts', import.meta.url), { type: 'module' });
        this.worker.onmessage = ({ data }) => {
            const wait = this.waiting.get(data.id);
            this.waiting.delete(data.id);
            if (data.error) wait?.reject(new Error(data.error));
            else wait?.resolve(data.result);
        };
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
            this.ask<NodeFeatures>(tile, node, 'stamps').then(stamps => this.keep(name, stamps), error => {
                // A tile that fails stamps nothing, as an absent one.
                console.warn(`Feature tile ${key(tile)}: ${error instanceof Error ? error.message : error}`);
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
        const body = await this.body(tile);
        if (!body) return ({ stamps: NOTHING, samples: NO_SAMPLES } as const)[what] as T;
        const id = this.next++;
        return new Promise<T>((resolve, reject) => {
            this.waiting.set(id, { resolve: resolve as (result: unknown) => void, reject });
            this.worker!.postMessage({ id, tile, node, body, what });
        });
    }

    private body(tile: Node): Promise<ArrayBuffer | undefined> {
        const name = key(tile);
        let body = this.bodies.get(name);
        if (body) {
            this.bodies.delete(name);
            this.bodies.set(name, body);
            return body;
        }
        body = this.tiles!(tile.level, tile.x, tile.y);
        this.bodies.set(name, body);
        body.then(data => {
            this.sizes.set(name, data?.byteLength ?? 0);
            this.bodyBytes += data?.byteLength ?? 0;
            for (const [old] of this.bodies) {
                if (this.bodyBytes <= BODIES) break;
                this.bodies.delete(old);
                this.bodyBytes -= this.sizes.get(old) ?? 0;
                this.sizes.delete(old);
            }
        }, () => this.bodies.delete(name));
        return body;
    }
}
