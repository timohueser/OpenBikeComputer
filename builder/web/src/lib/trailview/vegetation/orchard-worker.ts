// Makes the orchard trees of a zoom 14 feature tile off the main thread.
import { decodeFeatures } from '../stream/features';
import { orchardRows } from './orchard-rows';

self.onmessage = async ({ data: { id, tile, body } }: MessageEvent<{ id: number; tile: { x: number; y: number }; body: ArrayBuffer }>) => {
    try {
        let bytes = new Uint8Array(body);
        // A tile service may hand over the gzip of the archive as it is.
        if (bytes[0] === 0x1f && bytes[1] === 0x8b) bytes = new Uint8Array(await new Response(new Blob([bytes]).stream().pipeThrough(new DecompressionStream('gzip'))).arrayBuffer());
        const trees = orchardRows(decodeFeatures(bytes), tile);
        self.postMessage({ id, trees }, { transfer: [trees.buffer] });
    } catch (error) {
        self.postMessage({ id, error: error instanceof Error ? error.message : String(error) });
    }
};
