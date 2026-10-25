// Makes the orchard trees of a part of a feature tile off the main thread.
import { decodedTile } from '../stream/features';
import type { Node } from '../terrain/bins';
import { orchardRows } from './orchard-rows';

self.onmessage = async ({ data: { id, tile, owned, body } }: MessageEvent<{ id: number; tile: Node; owned: Node; body: ArrayBuffer }>) => {
    try {
        const trees = orchardRows(await decodedTile(`${tile.level}/${tile.x}/${tile.y}`, body), tile, owned);
        self.postMessage({ id, trees }, { transfer: [trees.buffer] });
    } catch (error) {
        self.postMessage({ id, error: error instanceof Error ? error.message : String(error) });
    }
};
