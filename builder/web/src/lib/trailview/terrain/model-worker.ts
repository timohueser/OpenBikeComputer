// The records worker (`ground=plan`): builds the records of cells in its own instance of the plan bridge.
// A worker blocked in a build cannot read its inbox, so a trap ends it (model.ts starts another).
import { imagePixels } from '../../planner/layers/terrain';
import { tilePages, type PageData } from '../stream/heights';
import init, { CellRequest, records } from '../plan/pkg/obc_ground_plan_bridge.js';

export type RecordsRequest =
    | { type: 'start'; module: WebAssembly.Module }
    | { type: 'build'; id: number; cell: { x: number; y: number }; tiles: { column: number; row: number; body: ArrayBuffer }[]; terrain: { z: number; x: number; y: number; body: ArrayBuffer }[]; pages: Uint32Array; words: number };

/** A built cell's records, or why it failed; `trapped` when the instance is not safe to use again. */
export type RecordsReply = { type: 'ready'; error?: string } | { id: number; words: Uint32Array; ms: number } | { id: number; error: string; trapped: boolean };

const post = (message: RecordsReply, transfer: Transferable[] = []) => self.postMessage(message, { transfer });
const message = (error: unknown) => (error instanceof Error ? error.message : String(error));

/** The plain bytes of a feature tile body: a tile service may hand over the gzip of the archive. */
async function plain(body: ArrayBuffer): Promise<Uint8Array> {
    const bytes = new Uint8Array(body);
    if (bytes[0] !== 0x1f || bytes[1] !== 0x8b) return bytes;
    return new Uint8Array(await new Response(new Blob([bytes]).stream().pipeThrough(new DecompressionStream('gzip'))).arrayBuffer());
}

async function build({ id, cell, tiles, terrain, pages, words }: Extract<RecordsRequest, { type: 'build' }>): Promise<void> {
    const request = new CellRequest(cell.x, cell.y);
    try {
        for (const { column, row, body } of tiles) request.add_tile(column, row, await plain(body));
        const decoded = new Map<string, (PageData | null)[]>();
        for (const { z, x, y, body } of terrain) {
            const rgba = await imagePixels(new Uint8Array(body));
            decoded.set(`${z}/${x}/${y}`, tilePages(rgba.data, rgba.width));
        }
        for (let i = 0; i < pages.length; i += words) {
            const page = decoded.get(`${pages[i + 3]}/${pages[i + 4]}/${pages[i + 5]}`)?.[pages[i + 6]];
            if (page) request.add_page(pages[i], pages[i + 1], pages[i + 2], Math.fround(page.offset), Math.fround(page.scale), page.texels);
        }
        const start = performance.now(), out = records(request);
        post({ id, words: out, ms: performance.now() - start }, [out.buffer]);
    } catch (error) {
        post({ id, error: message(error), trapped: error instanceof WebAssembly.RuntimeError || error instanceof RangeError });
    } finally {
        request.free();
    }
}

self.onmessage = async ({ data }: MessageEvent<RecordsRequest>) => {
    if (data.type === 'start') {
        try {
            await init({ module_or_path: data.module });
            post({ type: 'ready' });
        } catch (error) {
            post({ type: 'ready', error: message(error) });
        }
    } else await build(data);
};
