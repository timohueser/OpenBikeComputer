// Makes the house records of a zoom 14 feature tile off the main thread.
import { decodeFeatures } from '../stream/features';
import { tileHouses } from './house-records';

self.onmessage = async ({ data: { id, tile, body } }: MessageEvent<{ id: number; tile: { level: number; x: number; y: number }; body: ArrayBuffer }>) => {
    try {
        let bytes = new Uint8Array(body);
        // A tile service may hand over the gzip of the archive as it is.
        if (bytes[0] === 0x1f && bytes[1] === 0x8b) bytes = new Uint8Array(await new Response(new Blob([bytes]).stream().pipeThrough(new DecompressionStream('gzip'))).arrayBuffer());
        const result = tileHouses(decodeFeatures(bytes), tile);
        self.postMessage({ id, result }, { transfer: [result.records.buffer, result.outlines.buffer] });
    } catch (error) {
        self.postMessage({ id, error: error instanceof Error ? error.message : String(error) });
    }
};
