// The bake of `planner/trailview-stamps`: the stamp worker's output for the baked nodes of a feature tile, as
// node bytes. It runs in Node, where `self` is the global object that receives the worker's `onmessage`.
import { MeshoptEncoder } from 'meshoptimizer/encoder';
import './stamp-worker';
import { BAKED, EMPTY_MESHES, encodeNode, type PackedMesh } from './node-bytes';
import { OWNER, size, type Node } from './lod';
import { WATER_VERTEX } from './water-mesh';
import { DEEPEST } from '../stream/features';
import { STRUCTURE_VERTEX } from '../structures/mesh';
import { ANCHORED_VERTEX } from '../structures/anchored';
import type { NodeMeshes } from '../structures/node';
import type { NodeFeatures } from './stamps';
import type { Request } from './stamp-worker';

const worker = self as unknown as { onmessage: (event: { data: unknown }) => void; postMessage: (message: { result?: unknown; error?: string }) => void };

/** The nodes that read feature tile `tile` and that the layer holds: the BAKED levels, and the owner level for its structure meshes. */
function nodesOf(tile: Node): Node[] {
    const level = tile.level === DEEPEST ? OWNER : tile.level + 1;
    if (level !== OWNER && (level < BAKED.from || level > BAKED.to)) return [];
    const side = 2 ** (level - tile.level);
    return Array.from({ length: side * side }, (_, i) => ({ level, x: tile.x * side + (i % side), y: tile.y * side + Math.floor(i / side) }));
}

/** What the stamp worker posts for a job of `node` with the ground plan. */
function posted<T>(what: Request, tile: Node, node: Node, body: ArrayBuffer): Promise<T> {
    return new Promise((resolve, reject) => {
        worker.postMessage = ({ result, error }) => (error === undefined ? resolve(result as T) : reject(new Error(error)));
        worker.onmessage({ data: { id: 0, tile, node, body, what, plan: true } });
    });
}

/** The stamps of `node`, and the structure meshes of an owner node, as the stamp worker posts them. */
export const stamps = (tile: Node, node: Node, body: ArrayBuffer) => posted<NodeFeatures>('stamps', tile, node, body);
export const meshes = (tile: Node, node: Node, body: ArrayBuffer) => posted<NodeMeshes>('meshes', tile, node, body);

const code = (values: Uint32Array) => MeshoptEncoder.encodeIndexSequence(new Uint8Array(values.buffer, values.byteOffset, values.byteLength), values.length, 4);

/**
 * A mesh as the layer holds it (PackedMesh): its equal vertices once, in the order of their first use, and with
 * `middle` its first two int32 words as int16 from there, when all fit.
 */
function packMesh(mesh: { vertices: ArrayBuffer; indices: Uint32Array }, stride: number, middle: [number, number] | null): object {
    const [first, second, ...others] = Object.keys(mesh);
    if (first !== 'vertices' || second !== 'indices') throw new Error(`packMesh: keys ${Object.keys(mesh)}`);
    const { vertices, indices } = mesh, source = new Uint8Array(vertices), words = new DataView(vertices), n = source.byteLength / stride;
    const seen = new Map<string, number>(), of = new Uint32Array(n), distinct: number[] = [], latin1 = new TextDecoder('latin1');
    for (let k = 0; k < n; k++) {
        const name = latin1.decode(source.subarray(k * stride, (k + 1) * stride));
        of[k] = seen.get(name) ?? seen.set(name, distinct.push(k) - 1).get(name)!;
    }
    const near = (k: number, c: number) => Math.abs(words.getInt32(k * stride + 4 * c, true) - middle![c] + 0.5) < 32768;
    const xy = middle && distinct.every(k => near(k, 0) && near(k, 1)) ? middle : null;
    const w = distinct.length, s = xy ? stride - 4 : stride, out = new Uint8Array(w * s), view = new DataView(out.buffer);
    distinct.forEach((k, j) => {
        if (!xy) return out.set(source.subarray(k * stride, (k + 1) * stride), j * s);
        view.setInt16(j * s, words.getInt32(k * stride, true) - xy[0], true);
        view.setInt16(j * s + 2, words.getInt32(k * stride + 4, true) - xy[1], true);
        out.set(source.subarray(k * stride + 8, (k + 1) * stride), j * s + 4);
    });
    const $mesh: PackedMesh = { n, w, s, xy, v: MeshoptEncoder.encodeVertexBuffer(out, w, s), r: w === n ? null : code(of), ni: indices.length, i: code(indices) };
    return { $mesh, ...Object.fromEntries(others.map(name => [name, (mesh as unknown as Record<string, unknown>)[name]])) };
}

/** The structure meshes of an owner node, packed. */
function packMeshes(meshes: NodeMeshes, middle: [number, number]): object {
    const reach = ({ structures, anchored }: NodeMeshes['far']) => ({
        structures: structures && packMesh(structures, 4 * STRUCTURE_VERTEX, middle),
        anchored: anchored && packMesh(anchored, 4 * ANCHORED_VERTEX, null),
    });
    return { ...meshes, far: reach(meshes.far), near: reach(meshes.near) };
}

/**
 * The bytes of each baked node of feature tile `tile` with body `body`: a whole node with its water mesh packed,
 * or the packed structure meshes of each owner node that has any, and at the tile itself the u16 mask of those.
 */
export async function bakeTile(tile: Node, body: ArrayBuffer): Promise<[Node, Uint8Array][]> {
    await MeshoptEncoder.ready;
    const out: [Node, Uint8Array][] = [], empty = encodeNode(EMPTY_MESHES);
    let mask = 0;
    for (const [k, node] of nodesOf(tile).entries()) {
        const middle: [number, number] = [(node.x + 0.5) * size(node.level), (node.y + 0.5) * size(node.level)];
        if (node.level !== OWNER) {
            const result = await stamps(tile, node, body);
            out.push([node, encodeNode({ ...result, water: result.water && packMesh(result.water, 4 * WATER_VERTEX, middle) })]);
            continue;
        }
        // The layer holds an owner node's structure meshes alone: the bake asks for no other job of it.
        const made = await meshes(tile, node, body), bytes = encodeNode(made);
        if (bytes.length === empty.length && bytes.every((byte, i) => byte === empty[i])) continue;
        mask |= 1 << k;
        out.push([node, encodeNode(packMeshes(made, middle))]);
    }
    if (tile.level === DEEPEST) out.push([tile, new Uint8Array(new Uint16Array([mask]).buffer)]);
    return out;
}
