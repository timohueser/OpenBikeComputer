// The plan worker: builds cells of the ground plan in its own instance of the bridge. A worker blocked
// in a build cannot read its inbox, so a cancel or a trap ends it (plan.ts and model.ts start another).
import { imagePixels } from '../../planner/layers/terrain';
import { tilePages, type PageData } from '../stream/heights';
import { debugMesh, type DebugMesh, type Vocabulary } from './debug';
import init, { build, build_bytes, CellRequest, vocabulary, type CellPlan, type InitOutput, type MeshInput } from './pkg/obc_ground_plan_bridge.js';

/** Decoded stored tiles kept: 16 tiles of four 128 KiB pages, 8 MiB. */
const TILES = 16;
/** Mesh inputs kept for their cells' meshes: a town cell's faces take up to about 1 MiB of the heap. */
const INPUTS = 32;

/** A tile without `body` is one whose body went to the worker before: the bridge holds it parsed, or the answer is `Missing`. */
type Inputs = { cell: { x: number; y: number }; tiles: { column: number; row: number; body?: ArrayBuffer }[]; terrain: { z: number; x: number; y: number; body: ArrayBuffer }[]; pages: Uint32Array };

export type PlanRequest =
    | { type: 'start'; module: WebAssembly.Module }
    | ({ type: 'build'; id: number; debug: boolean } & Inputs)
    /** A cell's records and rim; with `mesh` the worker keeps its mesh input until a `mesh` or a `drop`. */
    | ({ type: 'cell'; id: number; mesh: boolean } & Inputs)
    /** The mesh of a kept cell from the rims of the cell and its eight neighbours, one after another (`rim` of a `CellBuilt`). */
    | { type: 'mesh'; id: number; cell: { x: number; y: number }; rims: Uint32Array }
    | { type: 'drop'; cell: { x: number; y: number } }
    | { type: 'time'; id: number; request: ArrayBuffer; runs: number };

/** A built cell; `digest` and `debug` only for a build with the debug draw. */
export interface Built { id: number; ok: true; stats: string; times: number[]; prepare: number; digest?: string; heap: number; debug?: DebugMesh }
/** A failed build; `trapped` when the instance is not safe to use again (a panic, out of memory). */
export interface Failed { id: number; ok: false; error: string; trapped: boolean }
/** The worker no longer holds feature tiles that a job named without a body, as column, row pairs: the job goes again with their bodies. */
export interface Missing { id: number; ok: true; missing: Uint32Array }
/** A cell's near records (the words that the GPU reads, whose labels its mesh's corners name), its rim and the build's milliseconds. */
export interface CellBuilt { id: number; ok: true; words: Uint32Array; rim: Uint32Array; ms: number; heap: number }
/**
 * A cell's near mesh (`mesh/mod.rs` of obc-ground-plan gives the bytes), or `gone` when the worker no
 * longer keeps its input; `ms` is the mesh stage.
 */
export type Meshed = { id: number; ok: true; mesh: ArrayBuffer; ms: number } | { id: number; ok: true; gone: true };
/**
 * The times of a request: the first run, then `runs` runs, each with its stage times and the total;
 * `mean`, the time of all `runs` over their count, holds where the clock is coarse (Safari: 1 ms).
 */
export interface Timed { id: number; ok: true; first: number[]; runs: number[][]; mean: number; digest: string; heap: number; stats: string }

let output: InitOutput | undefined;
let words: Vocabulary | undefined;
const decoded = new Map<string, Promise<(PageData | null)[]>>();
const kept = new Map<string, MeshInput>();

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

/**
 * The request of a cell's inputs, with the milliseconds of its gunzip and WebP decode. A tile without a body goes by
 * name; the caller checks `missing()` before the build, with no await between them.
 */
async function requestOf({ cell, tiles, terrain, pages }: Inputs): Promise<{ request: CellRequest; prepare: number }> {
    const start = performance.now();
    const plains = await Promise.all(tiles.map(({ body }) => body && plain(body)));
    const quadrants = new Map(await Promise.all(terrain.map(async ({ z, x, y, body }) => [`${z}/${x}/${y}`, await pagesOf(`${z}/${x}/${y}`, body)] as const)));
    const request = new CellRequest(cell.x, cell.y);
    try {
        tiles.forEach(({ column, row }, i) => {
            const bytes = plains[i];
            if (bytes) request.add_tile(column, row, bytes);
            else request.name_tile(column, row);
        });
        for (let i = 0; i < pages.length; i += words!.words.wanted) {
            const page = quadrants.get(`${pages[i + 3]}/${pages[i + 4]}/${pages[i + 5]}`)?.[pages[i + 6]];
            // StoredPage holds f32 offsets and scales.
            if (page) request.add_page(pages[i], pages[i + 1], pages[i + 2], Math.fround(page.offset), Math.fround(page.scale), page.texels);
        }
    } catch (error) {
        request.free();
        throw error;
    }
    return { request, prepare: performance.now() - start };
}

const failed = (id: number, error: unknown) => post({ id, ok: false, error: message(error), trapped: trapped(error) } satisfies Failed);
const nameOf = ({ x, y }: { x: number; y: number }) => `${x}-${y}`;

async function buildCell(job: Extract<PlanRequest, { type: 'build' }>): Promise<void> {
    const { id, cell, debug } = job;
    let plan: CellPlan | undefined;
    try {
        const made = await requestOf(job);
        plan = build(made.request, debug);
        const mesh = debug
            ? debugMesh({ samples: plan.samples(), ways: plan.ways(), points: plan.points(), crossings: plan.crossings(), vertices: plan.vertices(), faces: plan.faces(), rings: plan.rings(), ring_vertices: plan.ring_vertices(), edges: plan.edges() }, words!, cell)
            : undefined;
        const built: Built = { id, ok: true, stats: plan.stats(), times: [...plan.times()], prepare: made.prepare, heap: output!.memory.buffer.byteLength, ...(debug ? { digest: plan.digest(), debug: mesh } : {}) };
        post(built, mesh ? [mesh.lines.buffer, mesh.fills.buffer] : []);
    } catch (error) {
        failed(id, error);
    } finally {
        plan?.free();
    }
}

/** Builds a cell for the records and the mesh: the faces' records and rim, and with `mesh` its mesh input kept. */
async function buildRecords(job: Extract<PlanRequest, { type: 'cell' }>): Promise<void> {
    let plan: CellPlan | undefined;
    try {
        const { request } = await requestOf(job);
        const missing = request.missing();
        if (missing.length) {
            request.free();
            return post({ id: job.id, ok: true, missing } satisfies Missing, [missing.buffer]);
        }
        const start = performance.now();
        // The build takes the request: nothing frees it after.
        plan = build(request, false);
        const name = nameOf(job.cell);
        kept.get(name)?.free();
        kept.delete(name);
        const input = plan.mesh_input();
        const words = input.records(), rim = plan.rim();
        if (job.mesh) {
            kept.set(name, input);
            for (const [old, value] of kept) if (kept.size > INPUTS) {
                value.free();
                kept.delete(old);
            }
        } else input.free();
        post({ id: job.id, ok: true, words, rim, ms: performance.now() - start, heap: output!.memory.buffer.byteLength } satisfies CellBuilt, [words.buffer, rim.buffer]);
    } catch (error) {
        failed(job.id, error);
    } finally {
        plan?.free();
    }
}

function meshCell({ id, cell, rims }: Extract<PlanRequest, { type: 'mesh' }>): void {
    const name = nameOf(cell), input = kept.get(name);
    if (!input) return post({ id, ok: true, gone: true } satisfies Meshed);
    const start = performance.now();
    try {
        // A first mesh of an input always gives its bytes.
        const mesh = input.mesh(rims)!;
        post({ id, ok: true, mesh: mesh.buffer as ArrayBuffer, ms: performance.now() - start } satisfies Meshed, [mesh.buffer]);
    } catch (error) {
        failed(id, error);
    } finally {
        input.free();
        kept.delete(name);
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
        failed(id, error);
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
    else if (data.type === 'cell') await buildRecords(data);
    else if (data.type === 'mesh') meshCell(data);
    else if (data.type === 'drop') {
        kept.get(nameOf(data.cell))?.free();
        kept.delete(nameOf(data.cell));
    } else time(data);
};
