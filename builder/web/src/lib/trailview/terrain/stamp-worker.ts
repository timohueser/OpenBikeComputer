// Bins the features of a node's feature tile for stamp.wgsl, and makes the audit samples of a node, off the main thread.
import { decodeFeatures, type Feature } from '../stream/features';
import { auditSamples } from './audit-samples';
import { nodeStamps, type Node } from './bins';

/** Decoded tiles kept, by name. */
const KEPT = 24;
const tiles = new Map<string, Feature[]>();

async function decoded(name: string, body: ArrayBuffer): Promise<Feature[]> {
    let features = tiles.get(name);
    if (features) {
        tiles.delete(name);
    } else {
        let bytes = new Uint8Array(body);
        // A tile service may hand over the gzip of the archive as it is.
        if (bytes[0] === 0x1f && bytes[1] === 0x8b) bytes = new Uint8Array(await new Response(new Blob([bytes]).stream().pipeThrough(new DecompressionStream('gzip'))).arrayBuffer());
        features = decodeFeatures(bytes);
    }
    tiles.set(name, features);
    if (tiles.size > KEPT) tiles.delete(tiles.keys().next().value!);
    return features;
}

self.onmessage = async ({ data: { id, tile, node, body, baked, samples } }: MessageEvent<{ id: number; tile: Node; node: Node; body: ArrayBuffer; baked: boolean; samples: boolean }>) => {
    try {
        const features = await decoded(`${tile.level}/${tile.x}/${tile.y}`, body);
        if (samples) {
            const result = auditSamples(features, tile, node);
            self.postMessage({ id, result }, { transfer: [result.buffer] });
            return;
        }
        const result = nodeStamps(features, tile, node, baked);
        self.postMessage({ id, result }, { transfer: result.data ? [result.data.buffer] : [] });
    } catch (error) {
        self.postMessage({ id, error: error instanceof Error ? error.message : String(error) });
    }
};
