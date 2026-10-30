// Bins the features of a node's feature tile for stamp.wgsl and makes the node's water mesh and audit
// samples, off the main thread.
import { decodedTile } from '../stream/features';
import { auditSamples, waterLevels } from './audit-samples';
import { nodeStamps, type Node } from './bins';
import { waterMesh } from './water-mesh';
import { floorsOf, portalBoxes, structureMeshes } from '../structures/node';

export type Request = 'stamps' | 'samples';

self.onmessage = async ({ data: { id, tile, node, body, what, corridors } }: MessageEvent<{ id: number; tile: Node; node: Node; body: ArrayBuffer; what: Request; corridors: boolean }>) => {
    try {
        const features = await decodedTile(`${tile.level}/${tile.x}/${tile.y}`, body);
        if (what === 'samples') {
            const result = { samples: auditSamples(features, tile, node), ...waterLevels(features, tile) };
            self.postMessage({ id, result }, { transfer: [result.samples.buffer] });
        } else {
            const result = { ...nodeStamps(features, tile, node), water: waterMesh(features, tile, node), meshes: structureMeshes(features, tile, node, corridors), floors: floorsOf(features, tile, node), portals: portalBoxes(features, tile, node) };
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
