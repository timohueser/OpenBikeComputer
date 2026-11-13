// Bins the features of a node's feature tile for stamp.wgsl and makes the node's water mesh, an owner node's
// structure meshes and the audit samples, off the main thread.
import { decodedTile, DEEPEST, heldTile, pinTile, tileBudget, wantTiles, type Feature } from '../stream/features';
import { auditSamples, waterLevels } from './audit-samples';
import { nodeStamps, type Node } from './bins';
import { waterMesh } from './water-mesh';
import { floorsOf, portalBoxes, structureMeshes } from '../structures/node';

/** What a job makes of a node: its stamps with its water, floors and portals, an owner node's structure meshes, or its audit samples. */
export type Request = 'stamps' | 'meshes' | 'samples';

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
 * A request without `body` reads the tile that an earlier request brought; where the worker no longer holds it, the
 * answer is `missing`. `wanted` names the tiles of the nodes on their way, still wished or without their structure
 * meshes, and of the near levels' nodes that the last frame read: the worker keeps them past its budget.
 */
self.onmessage = async ({ data: { id, tile, node, body, what, plan, wanted = [] } }: MessageEvent<{ id: number; tile: Node; node: Node; body?: ArrayBuffer; what: Request; plan: boolean; wanted?: string[] }>) => {
    wantTiles(wanted);
    try {
        const held = tileFeatures(tile, body);
        if (!held) return void self.postMessage({ id, missing: true });
        const features = await held;
        if (what === 'samples') {
            const result = { samples: auditSamples(features, tile, node), ...waterLevels(features, tile) };
            self.postMessage({ id, result }, { transfer: [result.samples.buffer] });
        } else if (what === 'meshes') {
            const result = structureMeshes(features, tile, node, plan), { far, near, anchors, points } = result;
            const buffers = [far.structures, far.anchored, near.structures, near.anchored].flatMap(mesh => (mesh ? [mesh.vertices, mesh.indices.buffer] : []));
            self.postMessage({ id, result }, { transfer: [...buffers, ...(anchors ? [anchors] : []), ...(points ? [points.buffer] : [])] });
        } else {
            // The structure meshes are a job of their own (`meshes`): a node's stamps never wait for them.
            const result = { ...nodeStamps(features, tile, node, plan), water: waterMesh(features, tile, node), meshes: null, floors: floorsOf(features, tile, node), portals: portalBoxes(features, tile, node) };
            const water = result.water ? [result.water.vertices, result.water.indices.buffer] : [];
            self.postMessage({ id, result }, { transfer: [...(result.data ? [result.data.buffer] : []), ...water, ...[result.floors, result.portals].flatMap(words => (words ? [words.buffer] : []))] });
        }
    } catch (error) {
        self.postMessage({ id, error: error instanceof Error ? error.message : String(error) });
    }
};
