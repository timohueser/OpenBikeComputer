// Builds the corridor meshes of the owners off the main thread and off the stamps' worker: terrain never waits for roads.
import { chargeTile, decodedTile, tileBudget } from '../stream/features';
import type { Node } from '../terrain/bins';
import { planBytes, roadMesh } from './mesh';

/** The heap of the tiles that this worker keeps, with their shapes and plans: room for the tiles about an eye in all but the densest town. */
tileBudget(64 * 2 ** 20);

self.onmessage = async ({ data: { id, tile, owner, body } }: MessageEvent<{ id: number; tile: Node; owner: Node; body: ArrayBuffer }>) => {
    try {
        const name = `${tile.level}/${tile.x}/${tile.y}`, list = await decodedTile(name, body), mesh = roadMesh(list, tile, owner);
        chargeTile(name, planBytes(list, tile));
        self.postMessage({ id, mesh }, { transfer: mesh.pieces.flatMap(({ vertices, indices }) => [vertices, indices.buffer]) });
    } catch (error) {
        self.postMessage({ id, error: error instanceof Error ? error.message : String(error) });
    }
};
