import { featureTile, type NodeStamps } from './bins';
import { key, type Node } from './lod';
import type { StampSource } from './pages';

/** Feature tile bodies kept, and binned nodes kept. */
const BODIES = 32 * 2 ** 20, NODES = 4096;
/** Requests to the worker in flight at most. */
const REQUESTS = 8;
const NOTHING: NodeStamps = { data: null, heights: false, water: false };

/** The body of feature tile z/x/y, undefined where it is absent. */
export type FeatureTiles = (z: number, x: number, y: number) => Promise<ArrayBuffer | undefined>;

/**
 * The binned features of each node, from the feature tiles, binned in a worker. A node without a
 * feature tile above it, or without a feature archive, stamps nothing.
 */
export class Stamps implements StampSource {
    private readonly nodes = new Map<string, NodeStamps>();
    private readonly pending = new Set<string>();
    private readonly bodies = new Map<string, Promise<ArrayBuffer | undefined>>();
    private readonly sizes = new Map<string, number>();
    private bodyBytes = 0;
    private readonly worker?: Worker;
    private readonly waiting = new Map<number, { resolve: (result: unknown) => void; reject: (error: Error) => void }>();
    private next = 0;
    failed = 0;

    constructor(private readonly tiles: FeatureTiles | undefined, readonly baked: boolean, private readonly changed: () => void) {
        if (!tiles) return;
        this.worker = new Worker(new URL('./stamp-worker.ts', import.meta.url), { type: 'module' });
        this.worker.onmessage = ({ data }) => {
            const wait = this.waiting.get(data.id);
            this.waiting.delete(data.id);
            if (data.error) wait?.reject(new Error(data.error));
            else wait?.resolve(data.result);
        };
    }

    /** The node's stamps, undefined while they are on their way. */
    get(node: Node): NodeStamps | undefined {
        const tile = featureTile(node, this.baked);
        if (!this.tiles || !tile) return NOTHING;
        const name = key(node), known = this.nodes.get(name);
        if (known) {
            this.nodes.delete(name);
            this.nodes.set(name, known);
            return known;
        }
        if (!this.pending.has(name) && this.pending.size < REQUESTS) {
            this.pending.add(name);
            this.ask<NodeStamps>(tile, node, false).then(stamps => this.keep(name, stamps), error => {
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

    /** The audit samples of a node, as audit.wgsl reads them. */
    async samples(node: Node): Promise<Uint32Array> {
        const tile = featureTile(node, this.baked);
        return this.tiles && tile ? this.ask<Uint32Array>(tile, node, true) : new Uint32Array(0);
    }

    private keep(name: string, stamps: NodeStamps): void {
        this.nodes.set(name, stamps);
        if (this.nodes.size > NODES) this.nodes.delete(this.nodes.keys().next().value!);
    }

    private async ask<T>(tile: Node, node: Node, samples: boolean): Promise<T> {
        const body = await this.body(tile);
        if (!body) return (samples ? new Uint32Array(0) : NOTHING) as T;
        const id = this.next++;
        return new Promise<T>((resolve, reject) => {
            this.waiting.set(id, { resolve: resolve as (result: unknown) => void, reject });
            this.worker!.postMessage({ id, tile, node, body, baked: this.baked, samples });
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
