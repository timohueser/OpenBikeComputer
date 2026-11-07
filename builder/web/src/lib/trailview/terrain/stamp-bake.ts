// The bake of `planner/trailview-stamps`: the stamp worker's output for the baked nodes of a feature tile, as
// node bytes. It runs in Node, where `self` is the global object that receives the worker's `onmessage`.
import './stamp-worker';
import { BAKED, encodeNode } from './node-bytes';
import { OWNER, type Node } from './lod';
import { DEEPEST } from '../stream/features';
import type { NodeFeatures } from './stamps';

const worker = self as unknown as { onmessage: (event: { data: unknown }) => void; postMessage: (message: { result?: NodeFeatures; error?: string }) => void };

/** The nodes that read feature tile `tile` and that the layer holds: the BAKED levels, and the owner level for its structure meshes. */
function nodesOf(tile: Node): Node[] {
    const level = tile.level === DEEPEST ? OWNER : tile.level + 1;
    if (level !== OWNER && (level < BAKED.from || level > BAKED.to)) return [];
    const side = 2 ** (level - tile.level);
    return Array.from({ length: side * side }, (_, i) => ({ level, x: tile.x * side + (i % side), y: tile.y * side + Math.floor(i / side) }));
}

/** What the stamp worker posts for `node` with the ground plan. */
export function stamps(tile: Node, node: Node, body: ArrayBuffer): Promise<NodeFeatures> {
    return new Promise((resolve, reject) => {
        worker.postMessage = ({ result, error }) => (error === undefined ? resolve(result!) : reject(new Error(error)));
        worker.onmessage({ data: { id: 0, tile, node, body, what: 'stamps', plan: true } });
    });
}

/** The bytes of each baked node of feature tile `tile` with body `body`: a whole node, or the structure meshes of an owner node. */
export async function bakeTile(tile: Node, body: ArrayBuffer): Promise<[Node, Uint8Array][]> {
    const out: [Node, Uint8Array][] = [];
    for (const node of nodesOf(tile)) {
        const result = await stamps(tile, node, body);
        out.push([node, encodeNode(node.level === OWNER ? result.meshes : result)]);
    }
    return out;
}
