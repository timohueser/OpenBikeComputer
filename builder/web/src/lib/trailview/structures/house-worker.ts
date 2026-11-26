// Makes the house records of a part of a feature tile off the main thread.
import { decodedTile, tileBudget } from '../stream/features';
import type { Node } from '../terrain/bins';
import { tileHouses } from './house-records';

/** `bytes` is the worker's cap of decoded feature tiles. */
self.onmessage = async ({ data: { id, tile, owned, body, bytes } }: MessageEvent<{ id: number; tile: Node; owned: Node; body: ArrayBuffer; bytes: number }>) => {
    tileBudget(bytes);
    try {
        const result = tileHouses(await decodedTile(`${tile.level}/${tile.x}/${tile.y}`, body), tile, owned);
        self.postMessage({ id, result }, { transfer: [result.records.buffer, result.outlines.buffer] });
    } catch (error) {
        self.postMessage({ id, error: error instanceof Error ? error.message : String(error) });
    }
};
