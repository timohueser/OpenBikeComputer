// Builds the corridor meshes of the owners off the main thread and off the stamps' worker: terrain never waits for roads.
import { decodedTile } from '../stream/features';
import type { Node } from '../terrain/bins';
import { roadMesh } from './mesh';

self.onmessage = async ({ data: { id, tile, owner, body } }: MessageEvent<{ id: number; tile: Node; owner: Node; body: ArrayBuffer }>) => {
    try {
        const mesh = roadMesh(await decodedTile(`${tile.level}/${tile.x}/${tile.y}`, body), tile, owner);
        self.postMessage({ id, mesh }, { transfer: mesh.pieces.flatMap(({ vertices, indices }) => [vertices, indices.buffer]) });
    } catch (error) {
        self.postMessage({ id, error: error instanceof Error ? error.message : String(error) });
    }
};
