// The plan worker: builds cells of the ground plan in its own instance of the bridge. A worker blocked
// in a build cannot read its inbox, so a cancel or a trap ends it (plan.ts starts another).
import { imagePixels } from '../../planner/layers/terrain';
import { tilePages, type PageData } from '../stream/heights';
import { debugMesh, type DebugMesh, type Vocabulary } from './debug';
import init, { build, build_bytes, CellRequest, vocabulary, type CellPlan, type InitOutput, type MeshInput } from './pkg/obc_ground_plan_bridge.js';
import { around, due } from './wait';

/** Decoded stored tiles kept: 16 tiles of four 128 KiB pages, 8 MiB. */
const TILES = 16;

/**
 * `cells`, as `x-y` names: `wanted`, the cells in range (the worker keeps their mesh inputs); `coming`,
 * the wanted cells that the plan will still ask for (a mesh waits for their rims); `kept`, the cells
 * whose records the plan keeps (the worker keeps their rims and those of the wanted cells).
 */
export type PlanRequest =
    | { type: 'start'; module: WebAssembly.Module }
    | { type: 'build'; id: number; cell: { x: number; y: number }; tiles: { column: number; row: number; body: ArrayBuffer }[]; terrain: { z: number; x: number; y: number; body: ArrayBuffer }[]; pages: Uint32Array; debug: boolean }
    | { type: 'cells'; wanted: string[]; coming: string[]; kept: string[] }
    | { type: 'time'; id: number; request: ArrayBuffer; runs: number };

/**
 * The near mesh of a cell (`mesh/mod.rs` of obc-ground-plan gives the bytes) with the near records whose
 * labels its corners name, or why it failed, or `gone`: the cell's mesh does not fit its neighbours and
 * the worker cannot mesh it again, so the plan builds it again when it wants it; `ms` is the mesh stage.
 */
export type Meshed = { cell: { x: number; y: number }; ms: number } & ({ mesh: ArrayBuffer; records: ArrayBuffer } | { error: string } | { gone: true });
/**
 * The meshes that a build or a `cells` message made or changed; a later mesh of a cell replaces the earlier
 * one. `trapped` when a mesh left the instance not safe to use again.
 */
export interface Meshes { type: 'meshes'; meshes: Meshed[]; trapped: boolean }
/** A built cell; `digest` and `debug` only for a build with the debug draw. */
export interface Built { id: number; ok: true; stats: string; times: number[]; prepare: number; digest?: string; heap: number; debug?: DebugMesh }
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
/** The rims of the built cells that the plan keeps; an empty one where the build or the mesh failed, so the neighbours read the cell as absent. */
const rims = new Map<string, Uint32Array>();
/** The mesh inputs of the built cells that the plan wants. */
const inputs = new Map<string, MeshInput>();
/** The cells with a mesh that fits their neighbours' rims. */
const meshed = new Set<string>();
const NO_RIM = new Uint32Array(0);
let coming = new Set<string>();
let broken = false;

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

const xy = (name: string) => {
    const [x, y] = name.split('-').map(Number);
    return { x, y };
};
const same = (a: Uint32Array, b: Uint32Array) => a.length === b.length && a.every((w, i) => w === b[i]);

/**
 * Sets or drops a cell's rim. When it changed, the neighbours with a mesh input mesh again. A new or
 * changed rim makes a neighbour without its mesh input (the plan wants it no more) gone: its mesh does
 * not fit. A dropped rim does not: the plan draws neither cell.
 */
function setRim(name: string, rim: Uint32Array | undefined, out: Meshed[]): void {
    const old = rims.get(name);
    if (rim) rims.set(name, rim);
    else rims.delete(name);
    if (old && rim ? same(old, rim) : old === rim) return;
    for (const n of around(name)) {
        if (n === name) continue;
        if (inputs.has(n)) mesh(n, out);
        else if (rim && rims.get(n)?.length) gone(n, out);
    }
}

function gone(name: string, out: Meshed[]): void {
    meshed.delete(name);
    out.push({ cell: xy(name), ms: 0, gone: true });
}

/** Meshes a cell when it is due and what it reads changed; a cell whose mesh fails counts as a failed build. */
function mesh(name: string, out: Meshed[]): void {
    const input = inputs.get(name);
    if (!input || broken) return;
    // A cell that waits keeps its last mesh, which may not fit the rims there are now.
    if (!due([name], rims, coming).length) return void meshed.delete(name);
    const start = performance.now();
    try {
        const parts = around(name).flatMap(n => (rims.has(n) ? [rims.get(n)!] : []));
        const all = new Uint32Array(parts.reduce((sum, p) => sum + p.length, 0));
        parts.reduce((at, p) => (all.set(p, at), at + p.length), 0);
        const bytes = input.mesh(all);
        meshed.add(name);
        if (!bytes) return;
        out.push({ cell: xy(name), ms: performance.now() - start, mesh: bytes.buffer as ArrayBuffer, records: input.records().buffer as ArrayBuffer });
    } catch (error) {
        out.push({ cell: xy(name), ms: performance.now() - start, error: message(error) });
        if (trapped(error)) broken = true;
        else settle(name, undefined, out);
    }
}

/** A built cell's rim and mesh input, or an empty rim for a failed build or mesh; then the meshes that it changes. */
function settle(name: string, plan: CellPlan | undefined, out: Meshed[]): void {
    inputs.get(name)?.free();
    inputs.delete(name);
    meshed.delete(name);
    if (plan) inputs.set(name, plan.mesh_input());
    setRim(name, plan ? plan.rim() : NO_RIM, out);
    mesh(name, out);
}

function send(out: Meshed[]): void {
    if (out.length) post({ type: 'meshes', meshes: out, trapped: broken } satisfies Meshes, out.flatMap(m => ('mesh' in m ? [m.mesh, m.records] : [])));
}

/**
 * The plan's cells: drops the rims of the cells that it neither wants nor keeps, meshes the cells that
 * waited for one that comes no more, and frees the mesh inputs of the cells that it does not want (one
 * without a fitting mesh is gone).
 */
function cells(message: Extract<PlanRequest, { type: 'cells' }>): void {
    const out: Meshed[] = [], wanted = new Set(message.wanted), kept = new Set(message.kept);
    coming = new Set(message.coming);
    for (const name of [...rims.keys()]) if (!kept.has(name) && !wanted.has(name)) setRim(name, undefined, out);
    for (const name of [...inputs.keys()]) {
        if (!meshed.has(name)) mesh(name, out);
        if (wanted.has(name)) continue;
        inputs.get(name)!.free();
        inputs.delete(name);
        if (!meshed.has(name) && rims.get(name)?.length) gone(name, out);
    }
    send(out);
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
        const mesh = debug
            ? debugMesh({ samples: plan.samples(), ways: plan.ways(), points: plan.points(), crossings: plan.crossings(), vertices: plan.vertices(), faces: plan.faces(), rings: plan.rings(), ring_vertices: plan.ring_vertices(), edges: plan.edges() }, words!, cell)
            : undefined;
        const out: Meshed[] = [];
        settle(`${cell.x}-${cell.y}`, plan, out);
        send(out);
        const built: Built = { id, ok: true, stats: plan.stats(), times: [...plan.times()], prepare, heap: output!.memory.buffer.byteLength, ...(debug ? { digest: plan.digest(), debug: mesh } : {}) };
        post(built, mesh ? [mesh.lines.buffer, mesh.fills.buffer] : []);
    } catch (error) {
        post({ id, ok: false, error: message(error), trapped: trapped(error) } satisfies Failed);
        const out: Meshed[] = [];
        if (!trapped(error)) settle(`${cell.x}-${cell.y}`, undefined, out);
        send(out);
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
    else if (data.type === 'cells') cells(data);
    else time(data);
};
