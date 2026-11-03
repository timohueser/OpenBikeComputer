// The plan worker: builds cells of the ground plan in its own instance of the bridge. A worker blocked
// in a build cannot read its inbox, so a cancel or a trap ends it (plan.ts starts another).
import { imagePixels } from '../../planner/layers/terrain';
import { tilePages, type PageData } from '../stream/heights';
import { debugMesh, type Vocabulary } from './debug';
import init, { build, build_bytes, CellRequest, vocabulary, type CellPlan, type InitOutput } from './pkg/obc_ground_plan_bridge.js';

/** Decoded stored tiles kept: 16 tiles of four 128 KiB pages, 8 MiB. */
const TILES = 16;

export type PlanRequest =
    | { type: 'start'; module: WebAssembly.Module }
    | { type: 'build'; id: number; cell: { x: number; y: number }; tiles: { column: number; row: number; body: ArrayBuffer }[]; terrain: { z: number; x: number; y: number; body: ArrayBuffer }[]; pages: Uint32Array; debug: boolean }
    | { type: 'time'; id: number; request: ArrayBuffer; runs: number };

/** A built cell; `digest` and `debug` only for a build with the debug draw. */
export interface Built { id: number; ok: true; stats: string; times: number[]; prepare: number; digest?: string; heap: number; debug?: { lines: Int32Array<ArrayBuffer>; fills: Int32Array<ArrayBuffer> } }
/** A failed build; `trapped` when the instance is not safe to use again (a panic, out of memory). */
export interface Failed { id: number; ok: false; error: string; trapped: boolean }
/**
 * The times of a request: the first run, then `runs` runs, each with its stage times and the total;
 * `mean`, the time of all `runs` over their count, holds where the clock is coarse (Safari: 1 ms).
 */
export interface Timed { id: number; ok: true; first: number[]; runs: number[][]; mean: number; digest: string; heap: number; stats: string }

let output: InitOutput | undefined;
let words: Vocabulary | undefined;
const decoded = new Map<string, Promise<(PageData | null)[]>>();

const post = (message: unknown, transfer: Transferable[] = []) => self.postMessage(message, { transfer });
const message = (error: unknown) => (error instanceof Error ? error.message : String(error));
/** A trap leaves the instance broken; an error that the bridge returns does not. */
const trapped = (error: unknown) => error instanceof WebAssembly.RuntimeError || error instanceof RangeError;

/** The plain bytes of a feature tile body: a tile service may hand over the gzip of the archive. */
async function plain(body: ArrayBuffer): Promise<Uint8Array> {
    const bytes = new Uint8Array(body);
    if (bytes[0] !== 0x1f || bytes[1] !== 0x8b) return bytes;
    return new Uint8Array(await new Response(new Blob([bytes]).stream().pipeThrough(new DecompressionStream('gzip'))).arrayBuffer());
}

/** The pages of a stored tile, decoded as the decode worker does, so the codes are the GPU's. */
function pagesOf(name: string, body: ArrayBuffer): Promise<(PageData | null)[]> {
    let pages = decoded.get(name);
    if (pages) decoded.delete(name);
    else pages = imagePixels(new Uint8Array(body)).then(rgba => tilePages(rgba.data, rgba.width));
    pages.catch(() => decoded.delete(name));
    decoded.set(name, pages);
    for (const old of decoded.keys()) if (decoded.size > TILES) decoded.delete(old);
    return pages;
}

function stageTimes(plan: CellPlan): number[] {
    const times = [...plan.times()];
    return [...times, times.reduce((sum, t) => sum + t, 0)];
}

async function buildCell({ id, cell, tiles, terrain, pages, debug }: Extract<PlanRequest, { type: 'build' }>): Promise<void> {
    const start = performance.now();
    const request = new CellRequest(cell.x, cell.y);
    let plan: CellPlan | undefined;
    try {
        for (const { column, row, body } of tiles) request.add_tile(column, row, await plain(body));
        const quadrants = new Map(await Promise.all(terrain.map(async ({ z, x, y, body }) => [`${z}/${x}/${y}`, await pagesOf(`${z}/${x}/${y}`, body)] as const)));
        for (let i = 0; i < pages.length; i += words!.words.wanted) {
            const page = quadrants.get(`${pages[i + 3]}/${pages[i + 4]}/${pages[i + 5]}`)?.[pages[i + 6]];
            // StoredPage holds f32 offsets and scales.
            if (page) request.add_page(pages[i], pages[i + 1], pages[i + 2], Math.fround(page.offset), Math.fround(page.scale), page.texels);
        }
        const prepare = performance.now() - start;
        plan = build(request);
        const mesh = debug ? debugMesh({ samples: plan.samples(), ways: plan.ways(), points: plan.points(), crossings: plan.crossings() }, words!, cell) : undefined;
        const built: Built = { id, ok: true, stats: plan.stats(), times: [...plan.times()], prepare, heap: output!.memory.buffer.byteLength, ...(debug ? { digest: plan.digest(), debug: mesh } : {}) };
        post(built, mesh ? [mesh.lines.buffer, mesh.fills.buffer] : []);
    } catch (error) {
        post({ id, ok: false, error: message(error), trapped: trapped(error) } satisfies Failed);
    } finally {
        request.free();
        plan?.free();
    }
}

function time({ id, request, runs }: Extract<PlanRequest, { type: 'time' }>): void {
    const bytes = new Uint8Array(request), all: number[][] = [];
    let digest = '', stats = '', start = 0;
    try {
        for (let run = 0; run <= runs; run++) {
            if (run === 1) start = performance.now();
            const plan = build_bytes(bytes);
            all.push(stageTimes(plan));
            plan.free();
        }
        const mean = (performance.now() - start) / runs;
        const plan = build_bytes(bytes);
        try {
            [digest, stats] = [plan.digest(), plan.stats()];
        } finally {
            plan.free();
        }
        post({ id, ok: true, first: all[0], runs: all.slice(1), mean, digest, heap: output!.memory.buffer.byteLength, stats } satisfies Timed);
    } catch (error) {
        post({ id, ok: false, error: message(error), trapped: trapped(error) } satisfies Failed);
    }
}

self.onmessage = async ({ data }: MessageEvent<PlanRequest>) => {
    if (data.type === 'start') {
        try {
            output = await init({ module_or_path: data.module });
            words = JSON.parse(vocabulary());
            post({ type: 'ready' });
        } catch (error) {
            post({ type: 'ready', error: message(error) });
        }
    } else if (data.type === 'build') await buildCell(data);
    else time(data);
};
