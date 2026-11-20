// The tunnel parts of a node, from the part records of its feature tile (`tunnel-end`, `tunnel-tube`): their meshes,
// the boxes in which the ground does not draw, and the floor segments. host/obc-trailview-parts builds them, as WASM
// that this module inlines as a data: URL, so it loads the same way in a stamp worker and in the Node bake.
import wasm from './pkg/obc_trailview_parts_bridge_bg.wasm?url&inline';
import { initSync, meshes, volumes } from './pkg/obc_trailview_parts_bridge';
import type { Feature } from '../stream/features';
import { classPaint, worldPoint } from '../terrain/bins';
import type { Node } from '../terrain/lod';
import type { StructureMesh } from './mesh';

let ready = false;

/** Starts the bridge on the first call. */
function bridge(): void {
    if (ready) return;
    initSync({ module: Uint8Array.from(atob(wasm.slice(wasm.indexOf(',') + 1)), c => c.charCodeAt(0)) });
    ready = true;
}

/**
 * The input of the bridge for `node` of its feature tile: the node's square and the tile's part records in world
 * units, each chord with the surface code and the lines of a way of its class and surface.
 */
export function partInput(list: Feature[], tile: Node, node: Node): string {
    const side = 2 ** (30 - node.level), ends: object[] = [], chords: object[] = [];
    for (const { kind, tags, parts } of list) {
        for (const { xy, z } of kind === 'tunnel-end' || kind === 'tunnel-tube' ? parts : []) {
            if (!z) continue;
            if (kind === 'tunnel-end') ends.push({ at: worldPoint(tile, xy, 0), z: z[0], tags });
            else chords.push({ a: [...worldPoint(tile, xy, 0), z[0]], b: [...worldPoint(tile, xy, 1), z[1]], tags, ...classPaint(tags.class, tags.surface) });
        }
    }
    return JSON.stringify({ square: [node.x * side, node.y * side, (node.x + 1) * side, (node.y + 1) * side], ends, chords });
}

/** The meshes of the parts that the input's square draws; null with none. */
export function partMeshes(input: string): StructureMesh | null {
    bridge();
    const out = meshes(input), n = out[0];
    return out.length > 1 ? { vertices: out.slice(1, 1 + n).buffer, indices: out.slice(1 + n) } : null;
}

/** The boxes (`BOX` of obc-trailview-parts, 12 words each) and the floor segments (`FLOOR` words) of the parts near the input's square. */
export function partVolumes(input: string): { boxes: Float64Array<ArrayBuffer>; floors: Float64Array<ArrayBuffer> } {
    bridge();
    const out = volumes(input), n = out[0];
    return { boxes: out.slice(1, 1 + n), floors: out.slice(1 + n) };
}
