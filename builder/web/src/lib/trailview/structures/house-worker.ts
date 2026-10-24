// Makes the house records of a part of a feature tile off the main thread.
import { decodedTile } from '../stream/features';
import type { Node } from '../terrain/bins';
import { tileHouses } from './house-records';

self.onmessage = async ({ data: { id, tile, owned, body } }: MessageEvent<{ id: number; tile: Node; owned: Node; body: ArrayBuffer }>) => {
    try {
        const result = tileHouses(await decodedTile(`${tile.level}/${tile.x}/${tile.y}`, body), tile, owned);
        self.postMessage({ id, result }, { transfer: [result.records.buffer, result.outlines.buffer] });
    } catch (error) {
        self.postMessage({ id, error: error instanceof Error ? error.message : String(error) });
    }
};
