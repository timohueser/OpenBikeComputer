// Decodes stored terrain tiles into pages, off the main thread.
import { imagePixels } from '../../planner/layers/terrain';
import { tilePages, type PageFormat } from './heights';

self.onmessage = async ({ data: { id, body, format } }: MessageEvent<{ id: number; body: ArrayBuffer; format: PageFormat }>) => {
    try {
        const rgba = await imagePixels(new Uint8Array(body));
        const pages = tilePages(rgba.data, rgba.width, format);
        self.postMessage({ id, pages }, { transfer: pages.flatMap(page => (page ? [page.texels.buffer] : [])) });
    } catch (error) {
        self.postMessage({ id, error: error instanceof Error ? error.message : String(error) });
    }
};
