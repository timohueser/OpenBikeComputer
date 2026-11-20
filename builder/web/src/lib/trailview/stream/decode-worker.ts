// Decodes stored terrain tiles into pages, off the main thread.
import { terrainPixels } from './exact-pixels';
import { tilePages } from './heights';

self.onmessage = async ({ data: { id, body } }: MessageEvent<{ id: number; body: ArrayBuffer }>) => {
    try {
        const { data, size } = await terrainPixels(body);
        const pages = tilePages(data, size);
        self.postMessage({ id, pages }, { transfer: pages.flatMap(page => (page ? [page.texels.buffer] : [])) });
    } catch (error) {
        self.postMessage({ id, error: error instanceof Error ? error.message : String(error) });
    }
};
