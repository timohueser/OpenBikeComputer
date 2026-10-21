// Bins the features of a node's feature tile for stamp.wgsl and makes the node's water mesh and audit
// samples, off the main thread.
import { decodeFeatures, type Feature } from '../stream/features';
import { auditSamples, waterLevels } from './audit-samples';
import { nodeStamps, type Node } from './bins';
import { waterMesh } from './water-mesh';

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

export type Request = 'stamps' | 'samples';

self.onmessage = async ({ data: { id, tile, node, body, what } }: MessageEvent<{ id: number; tile: Node; node: Node; body: ArrayBuffer; what: Request }>) => {
    try {
        const features = await decoded(`${tile.level}/${tile.x}/${tile.y}`, body);
        if (what === 'samples') {
            const result = { samples: auditSamples(features, tile, node), ...waterLevels(features, tile) };
            self.postMessage({ id, result }, { transfer: [result.samples.buffer] });
        } else {
            const result = { ...nodeStamps(features, tile, node), water: waterMesh(features, tile, node) };
            self.postMessage({ id, result }, { transfer: [...(result.data ? [result.data.buffer] : []), ...(result.water ? [result.water.vertices, result.water.indices.buffer] : [])] });
        }
    } catch (error) {
        self.postMessage({ id, error: error instanceof Error ? error.message : String(error) });
    }
};
