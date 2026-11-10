// Bins the features of a node's feature tile for stamp.wgsl and makes the node's water mesh and audit
// samples, off the main thread.
import { decodedTile, DEEPEST, heldTile, pinTile, tileBudget, type Feature } from '../stream/features';
import { auditSamples, waterLevels } from './audit-samples';
import { nodeStamps, type Node } from './bins';
import { waterMesh } from './water-mesh';
import { floorsOf, portalBoxes, structureMeshes } from '../structures/node';

export type Request = 'stamps' | 'samples';

/**
 * The 2 × 2 tiles of each zoom coarser than 13 read last, pinned in the byte budget: an eye near a tile
 * corner reads all four, and the shapes of a zoom 8 tile take about half a second. The pins hold at most
 * a third of the budget, so the deep tiles keep the 32 MiB of the other workers.
 */
const COARSE = 4;
tileBudget(48 * 2 ** 20);
const coarse = new Map<number, string[]>();

/** The features of a tile from its body, or as the worker still holds them without one; undefined where it holds none. */
function tileFeatures(tile: Node, body?: ArrayBuffer): Promise<Feature[]> | undefined {
    const name = `${tile.level}/${tile.x}/${tile.y}`, features = body ? decodedTile(name, body) : heldTile(name);
    if (features && tile.level < DEEPEST) {
        const last = [...(coarse.get(tile.level) ?? []).filter(other => other !== name), name];
        pinTile(name, true);
        while (last.length > COARSE) pinTile(last.shift()!, false);
        coarse.set(tile.level, last);
    }
    return features;
}

/**
 * Without `meshes` the node's structure meshes are null: the stamp layer holds them. A request without `body` reads the
 * tile that an earlier request brought; where the worker no longer holds it, the answer is `missing`.
 */
self.onmessage = async ({ data: { id, tile, node, body, what, plan, meshes = true } }: MessageEvent<{ id: number; tile: Node; node: Node; body?: ArrayBuffer; what: Request; plan: boolean; meshes?: boolean }>) => {
    try {
        const held = tileFeatures(tile, body);
        if (!held) return void self.postMessage({ id, missing: true });
        const features = await held;
        if (what === 'samples') {
            const result = { samples: auditSamples(features, tile, node), ...waterLevels(features, tile) };
            self.postMessage({ id, result }, { transfer: [result.samples.buffer] });
        } else {
            const result = { ...nodeStamps(features, tile, node, plan), water: waterMesh(features, tile, node), meshes: meshes ? structureMeshes(features, tile, node) : null, floors: floorsOf(features, tile, node), portals: portalBoxes(features, tile, node) };
            const kinds = result.meshes ? [result.meshes.far.structures, result.meshes.far.anchored, result.meshes.near.structures, result.meshes.near.anchored] : [];
            const buffers = [result.water, ...kinds].flatMap(mesh => (mesh ? [mesh.vertices, mesh.indices.buffer] : []));
            if (result.meshes?.anchors) buffers.push(result.meshes.anchors);
            if (result.meshes?.points) buffers.push(result.meshes.points.buffer);
            self.postMessage({ id, result }, { transfer: [...(result.data ? [result.data.buffer] : []), ...buffers, ...[result.floors, result.portals].flatMap(words => (words ? [words.buffer] : []))] });
        }
    } catch (error) {
        self.postMessage({ id, error: error instanceof Error ? error.message : String(error) });
    }
};
