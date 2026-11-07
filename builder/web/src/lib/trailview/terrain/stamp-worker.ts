// Bins the features of a node's feature tile for stamp.wgsl and makes the node's water mesh and audit
// samples, off the main thread.
import { decodedTile, DEEPEST, type Feature } from '../stream/features';
import { auditSamples, waterLevels } from './audit-samples';
import { nodeStamps, type Node } from './bins';
import { waterMesh } from './water-mesh';
import { floorsOf, portalBoxes, structureMeshes } from '../structures/node';

export type Request = 'stamps' | 'samples';

/**
 * The coarse tiles last read, kept past the byte budget: the shapes of a zoom 8 tile take about a
 * second, and the deep tiles between its nodes would push it out. Four hold the 2 × 2 tiles about a view.
 */
const COARSE = 4;
const coarse = new Map<string, Promise<Feature[]>>();

function tileFeatures(tile: Node, body: ArrayBuffer): Promise<Feature[]> {
    const name = `${tile.level}/${tile.x}/${tile.y}`;
    if (tile.level >= DEEPEST) return decodedTile(name, body);
    const features = coarse.get(name) ?? decodedTile(name, body);
    coarse.delete(name);
    coarse.set(name, features);
    for (const old of coarse.keys()) if (coarse.size > COARSE) coarse.delete(old);
    return features;
}

self.onmessage = async ({ data: { id, tile, node, body, what, plan } }: MessageEvent<{ id: number; tile: Node; node: Node; body: ArrayBuffer; what: Request; plan: boolean }>) => {
    try {
        const features = await tileFeatures(tile, body);
        if (what === 'samples') {
            const result = { samples: auditSamples(features, tile, node), ...waterLevels(features, tile) };
            self.postMessage({ id, result }, { transfer: [result.samples.buffer] });
        } else {
            const result = { ...nodeStamps(features, tile, node, plan), water: waterMesh(features, tile, node), meshes: structureMeshes(features, tile, node), floors: floorsOf(features, tile, node), portals: portalBoxes(features, tile, node) };
            const { far, near, anchors } = result.meshes;
            const meshes = [result.water, far.structures, far.anchored, near.structures, near.anchored].flatMap(mesh => (mesh ? [mesh.vertices, mesh.indices.buffer] : []));
            if (anchors) meshes.push(anchors);
            if (result.meshes.points) meshes.push(result.meshes.points.buffer);
            self.postMessage({ id, result }, { transfer: [...(result.data ? [result.data.buffer] : []), ...meshes, ...[result.floors, result.portals].flatMap(words => (words ? [words.buffer] : []))] });
        }
    } catch (error) {
        self.postMessage({ id, error: error instanceof Error ? error.message : String(error) });
    }
};
