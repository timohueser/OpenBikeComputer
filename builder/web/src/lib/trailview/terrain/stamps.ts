import { featureTile, type NodeStamps } from './bins';
import { key, type Node } from './lod';
import type { StampSource } from './pages';
import type { Request } from './stamp-worker';
import type { WaterMesh } from './water-mesh';

/** Feature tile bodies kept, binned nodes kept, and water meshes kept. */
const BODIES = 32 * 2 ** 20, NODES = 4096, MESHES = 256;
/** Requests to the worker in flight at most. */
const REQUESTS = 8;
const NOTHING: NodeStamps = { data: null, heights: false };

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
 * The binned features of each node and the water meshes of each feature tile, made in a worker. A node
 * without a feature tile above it, or without a feature archive, stamps nothing and has no water.
 */
export class Stamps implements StampSource {
    private readonly nodes = new Map<string, NodeStamps>();
    private readonly meshes = new Map<string, WaterMesh | null>();
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

    /** The node's stamps, undefined while they are on their way. */
    get(node: Node): NodeStamps | undefined {
        const tile = featureTile(node);
        if (!this.tiles || !tile) return NOTHING;
        const name = key(node), known = this.nodes.get(name);
        if (known) {
            this.nodes.delete(name);
            this.nodes.set(name, known);
            return known;
        }
        if (!this.pending.has(name) && this.pending.size < REQUESTS) {
            this.pending.add(name);
            this.ask<NodeStamps>(tile, node, 'stamps').then(stamps => this.keep(name, stamps), error => {
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

    /** The water mesh of a feature tile, null without water, undefined while it is on its way. */
    mesh(tile: Node): WaterMesh | null | undefined {
        const name = `water/${key(tile)}`, known = this.meshes.get(name);
        if (!this.tiles) return null;
        if (known !== undefined) return known;
        if (!this.pending.has(name) && this.pending.size < REQUESTS) {
            this.pending.add(name);
            this.ask<WaterMesh | null>(tile, tile, 'water').then(mesh => mesh, () => {
                this.failed++;
                return null;
            }).then(mesh => {
                this.meshes.set(name, mesh);
                if (this.meshes.size > MESHES) this.meshes.delete(this.meshes.keys().next().value!);
                this.pending.delete(name);
                this.changed();
            });
        }
        return undefined;
    }

    private keep(name: string, stamps: NodeStamps): void {
        this.nodes.set(name, stamps);
        if (this.nodes.size > NODES) this.nodes.delete(this.nodes.keys().next().value!);
    }

    private async ask<T>(tile: Node, node: Node, what: Request): Promise<T> {
        const body = await this.body(tile);
        if (!body) return ({ stamps: NOTHING, samples: NO_SAMPLES, water: null } as const)[what] as T;
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
