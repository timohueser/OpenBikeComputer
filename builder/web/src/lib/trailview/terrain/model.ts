// The records of the cells about the eye on the device (`ground=plan`): built in plan workers from the
// cells' inputs, kept nearest first within RECORDS_MB, in one texture of MODEL_ROW words per row that
// the stamp pass reads (model.wgsl). Row 0 holds a table of the cells about the eye; each block takes
// whole rows after it. The same build gives a cell's rim and, for the cells that the mesh wants, keeps
// the input of its mesh in its worker, which meshes it once the rims of its eight neighbours are here:
// each cell builds once.
import { stopped } from '../stream/terrain';
import { cellInputs, type StoredTiles } from '../plan/inputs';
import { HEAP_LIMIT } from '../plan/plan';
import init, { feature_tiles, vocabulary, wanted } from '../plan/pkg/obc_ground_plan_bridge.js';
import type { CellBuilt, Failed, Meshed, PlanRequest } from '../plan/plan-worker';
import { distances, key, type Node } from './lod';
import type { FeatureTiles } from './stamps';

const WASM = new URL('../plan/pkg/obc_ground_plan_bridge_bg.wasm', import.meta.url);
/** Words per row of the texture: MODEL_ROW of records.wgsl. */
export const MODEL_ROW = 4096;
/** The level of the nodes that are cells: zoom 16, NEAR of bins.ts. */
export const CELL_LEVEL = 16;
/**
 * The records' GPU memory. The cells under the wanted near pages come first, nearest first; a town
 * shrinks how far they reach. It holds every wanted cell of a town view of the fixture.
 */
export const RECORDS_MB = 32;
export const ROWS = (RECORDS_MB * 2 ** 20) / (MODEL_ROW * 4);
/** The table's first word, as M_TABLE of model.wgsl, and its cells per side about the eye. */
const TABLE_MAGIC = 0x4f42540a;
const TABLE = 48;
/** Plan workers, and the builds that each holds: one runs and one waits. Two workers halve the time to a full view after a jump. */
const WORKERS = 2;
const JOBS = 2;
/** Rows that a block not built yet counts as, until blocks are known. */
const GUESS_ROWS = 16;

type Cell = {
    node: Node;
    distance: number;
    /** The build on its way, and whether it keeps the cell's mesh input. A built cell keeps its block while it builds again. */
    job?: number;
    keep?: boolean;
    rim?: Uint32Array;
    /** The worker that keeps the cell's mesh input, and the request of the mesh on its way. */
    worker?: number;
    mesh?: 'asked' | 'done';
    meshJob?: number;
    /** The labels of its block that are junctions with a stop arm (mouthLabels). */
    mouths?: Set<number>;
} & (
    // `rows` is the block's size once known. An `over` cell's block found no room: it asks again when there is.
    | { state: 'asked'; rows?: number }
    | { state: 'built'; first: number; rows: number }
    | { state: 'over'; rows: number }
    | { state: 'empty' }
    | { state: 'failed' });

export interface RecordsStats {
    /** Cells with a block, and their bytes on the device. */
    cells: number;
    bytes: number;
    /** Metres from the eye to the farthest cell with a block, and to the farthest wanted cell. */
    radius: number;
    wanted: number;
    building: number;
    /** The median build of a cell in a worker, and of a mesh, in ms. */
    build: number;
    mesh: number;
    restarts: number;
    /** The largest heap of a worker, in bytes. */
    heap: number;
    error?: string;
}

/** Row ranges [first, count], sorted, with touching ranges joined. */
const merged = (ranges: [number, number][]) => [...ranges].sort((a, b) => a[0] - b[0]).reduce<[number, number][]>((out, range) => {
    const last = out[out.length - 1];
    if (last && last[0] + last[1] === range[0]) last[1] += range[1];
    else out.push([...range]);
    return out;
}, []);

/** A cell's mesh (`mesh/mod.rs` of obc-ground-plan gives the bytes), the first word of its block in the texture and the stamp data of its corners. */
export interface CellMeshBytes { node: Node; bytes: ArrayBuffer; block: number; mouths: Set<number>; stamps: Uint32Array<ArrayBuffer> }

/** As LABEL, the junction owner and STOP_ARM of the crate's `faces::records`. */
const LABEL_WORDS = 12;
const OWNER_JUNCTION = 2;
const STOP_ARM = 0x80000000;

/** The labels of a records block that are junctions with a stop arm: their faces draw with the mouth's look (plan/pieces.ts). */
export function mouthLabels(words: Uint32Array): Set<number> {
    const out = new Set<number>(), first = words[5];
    for (let id = 0; id < words[2]; id++) {
        const l = first + id * LABEL_WORDS;
        if (((words[l] >>> 8) & 0xff) !== OWNER_JUNCTION) continue;
        for (let k = 8; k < 12; k++) if (words[l + k] !== 0xffffffff && (words[l + k] & STOP_ARM) !== 0) out.add(id);
    }
    return out;
}

/** The cell of a node of level CELL_LEVEL or finer. */
export const cellOf = ({ level, x, y }: Node): Node => ({ level: CELL_LEVEL, x: x >> (level - CELL_LEVEL), y: y >> (level - CELL_LEVEL) });

const around = ({ x, y }: Node): Node[] => [-1, 0, 1].flatMap(dy => [-1, 0, 1].flatMap(dx => (dx || dy ? [{ level: CELL_LEVEL, x: x + dx, y: y + dy }] : [])));

interface Slot { worker: Worker; ready: boolean; jobs: Map<number, string> }

export class Records {
    private cells = new Map<string, Cell>();
    /** Free row ranges [first, count]; row 0 is the table. */
    private free: [number, number][] = [[1, ROWS - 1]];
    private uploads: { first: number; words: Uint32Array<ArrayBuffer> }[] = [];
    /** The cells whose block arrived or left since the last `takeChanged`. */
    private changed: Node[] = [];
    private target = new Set<string>();
    private meshWanted = new Set<string>();
    private origin: [number, number] = [-1, -1];
    private tableDirty = true;
    private pool: Slot[] = [];
    private module?: WebAssembly.Module;
    private words = 0;
    private next = 0;
    private times: number[] = [];
    private meshTimes: number[] = [];
    private restarts = 0;
    private error?: string;
    private wantedM = 0;
    private heap = 0;

    /** `meshed` takes each cell's mesh as it arrives, `unmeshed` each cell whose mesh leaves. */
    constructor(private readonly features: FeatureTiles | undefined, private readonly stored: StoredTiles, private readonly onChange: () => void, private readonly stop: AbortSignal,
        private readonly meshed: (mesh: CellMeshBytes) => void = () => {}, private readonly unmeshed: (node: Node) => void = () => {}) {
        stop.addEventListener('abort', () => this.pool.forEach(({ worker }) => worker.terminate()));
        if (!features) {
            this.error = 'the release has no feature tiles';
            return;
        }
        void this.load();
    }

    private async load(): Promise<void> {
        try {
            const module = await WebAssembly.compile(await (await fetch(WASM, { signal: this.stop })).arrayBuffer());
            await init({ module_or_path: module });
            this.words = JSON.parse(vocabulary()).words.wanted;
            this.module = module;
            for (let k = 0; k < WORKERS; k++) this.spawn(k);
        } catch (error) {
            if (stopped(error)) return;
            this.error = `the records bridge did not start: ${error instanceof Error ? error.message : error}`;
            this.onChange();
        }
    }

    private spawn(k: number): void {
        if (this.stop.aborted) return;
        const worker = new Worker(new URL('../plan/plan-worker.ts', import.meta.url), { type: 'module' });
        this.pool[k] = { worker, ready: false, jobs: new Map() };
        worker.onmessage = ({ data }: MessageEvent<{ type: 'ready'; error?: string } | CellBuilt | Meshed | Failed>) => this.receive(k, worker, data);
        worker.onerror = event => {
            if (worker !== this.pool[k].worker) return;
            this.error = `a records worker failed: ${event.message}`;
            this.pool[k].ready = false;
            this.onChange();
        };
        worker.postMessage({ type: 'start', module: this.module! } satisfies PlanRequest);
    }

    /** Ends worker k and starts another: its builds go back to not asked, its kept mesh inputs are gone. */
    private restart(k: number): void {
        const old = this.pool[k];
        old.worker.terminate();
        this.restarts++;
        for (const name of old.jobs.values()) {
            const cell = this.cells.get(name);
            if (cell?.state === 'asked') this.cells.delete(name);
            else if (cell) {
                cell.job = undefined;
                if (cell.mesh === 'asked') cell.mesh = undefined;
            }
        }
        for (const cell of this.cells.values()) if (cell.worker === k) cell.worker = undefined;
        this.spawn(k);
    }

    private receive(k: number, worker: Worker, data: { type: 'ready'; error?: string } | CellBuilt | Meshed | Failed): void {
        const slot = this.pool[k];
        if (worker !== slot.worker) return;
        if ('type' in data) {
            if (data.error) this.error = `the records worker did not start: ${data.error}`;
            else slot.ready = true;
            return this.onChange();
        }
        const name = slot.jobs.get(data.id);
        slot.jobs.delete(data.id);
        const cell = name === undefined ? undefined : this.cells.get(name);
        if (!data.ok) {
            console.warn(`Ground plan cell ${name}: ${data.error}`);
            if (cell?.job === data.id) {
                cell.job = undefined;
                if (cell.state === 'asked') Object.assign(cell, { state: 'failed' });
                else cell.mesh = 'done';
            } else if (cell?.mesh === 'asked' && cell.meshJob === data.id) {
                cell.mesh = 'done';
                cell.worker = undefined;
            }
            if (data.trapped) this.restart(k);
        } else if ('words' in data) {
            this.heap = Math.max(this.heap, data.heap);
            if (cell?.job === data.id) {
                this.times = [...this.times.slice(-31), data.ms];
                cell.job = undefined;
                cell.rim = data.rim;
                if (cell.keep && this.meshWanted.has(name!)) cell.worker = k;
                else if (cell.keep) worker.postMessage({ type: 'drop', cell: cell.node } satisfies PlanRequest);
                if (cell.state === 'asked') this.place(cell, data.words);
            }
            // A full heap ends the worker once it is idle: WASM memory never shrinks.
            if (data.heap > HEAP_LIMIT && !slot.jobs.size) this.restart(k);
        } else if (cell?.mesh === 'asked' && cell.meshJob === data.id) {
            cell.worker = undefined;
            if ('gone' in data) cell.mesh = undefined;
            else if (cell.state === 'built' && this.meshWanted.has(name!)) {
                this.meshTimes = [...this.meshTimes.slice(-31), data.ms];
                cell.mesh = 'done';
                this.meshed({ node: cell.node, bytes: data.mesh, block: cell.first * MODEL_ROW, mouths: cell.mouths ?? new Set(), stamps: data.stamps });
            } else cell.mesh = undefined;
        }
        this.onChange();
    }

    /**
     * The built cells that may leave for the block of `cell`, in order: those past the target and the mesh,
     * then the target's farther than it, each farthest first. The mesh cells and their rings stay: a ring
     * cell holds a rim that a mesh waits for.
     */
    private leaving(cell: Cell): [string, Cell][] {
        const kept = new Set([...this.meshWanted].flatMap(name => [name, ...around(nodeOf(name)).map(key)]));
        const past = (name: string) => !this.target.has(name) && !kept.has(name);
        return [...this.cells.entries()].filter(([name, c]) => c.state === 'built' && (past(name) || (c.distance > cell.distance && !kept.has(name))))
            .sort(([a, ca], [b, cb]) => Number(past(b)) - Number(past(a)) || cb.distance - ca.distance);
    }

    /** Whether a run of `rows` free rows exists once the `leaving` cells go. */
    private room(rows: number, leaving: [string, Cell][]): boolean {
        const taken = leaving.flatMap(([, c]): [number, number][] => (c.state === 'built' ? [[c.first, c.rows]] : []));
        return merged([...this.free, ...taken]).some(([, count]) => count >= rows);
    }

    /** Puts a block into free rows; the `leaving` cells go, in order, until it fits. Without room the cell is `over`. */
    private place(cell: Cell, words: Uint32Array): void {
        const rows = Math.ceil(words.length / MODEL_ROW), leaving = this.leaving(cell);
        if (!this.room(rows, leaving)) {
            Object.assign(cell, { state: 'over', rows });
            return;
        }
        let first = this.allocate(rows);
        while (first === undefined) {
            this.drop(...leaving.shift()!);
            first = this.allocate(rows);
        }
        Object.assign(cell, { state: 'built', first, rows, mouths: mouthLabels(words) });
        const padded = new Uint32Array(rows * MODEL_ROW);
        padded.set(words);
        this.uploads.push({ first, words: padded });
        this.changed.push(cell.node);
        this.tableDirty = true;
    }

    private allocate(rows: number): number | undefined {
        const k = this.free.findIndex(([, count]) => count >= rows);
        if (k < 0) return undefined;
        const [first, count] = this.free[k];
        if (count === rows) this.free.splice(k, 1);
        else this.free[k] = [first + rows, count - rows];
        return first;
    }

    private drop(name: string, cell: Cell): void {
        if (cell.state === 'built') {
            this.free = merged([...this.free, [cell.first, cell.rows]]);
            this.changed.push(cell.node);
            this.tableDirty = true;
        }
        this.unmesh(cell);
        this.cells.delete(name);
    }

    /** Takes a cell's mesh off, and frees its kept input. */
    private unmesh(cell: Cell): void {
        if (cell.mesh === 'done') this.unmeshed(cell.node);
        if (cell.worker !== undefined && cell.mesh !== 'asked') this.pool[cell.worker]?.worker.postMessage({ type: 'drop', cell: cell.node } satisfies PlanRequest);
        cell.mesh = undefined;
        cell.worker = undefined;
    }

    /**
     * Takes the cells under the frame's wanted near pages, nearest first: those whose blocks fit the
     * budget are the target, and the workers build the next of them. A block of unknown size counts as
     * the mean of the built ones. The cells of `mesh` get their meshes, and their neighbours their rims:
     * they build in one near-first order with the target, each neighbour as near as its mesh cell.
     */
    update(cells: { node: Node; distance: number }[], mesh: { node: Node; distance: number }[] = []): void {
        const meshNames = new Set(mesh.map(({ node }) => key(node)));
        for (const name of this.meshWanted) {
            const cell = this.cells.get(name);
            if (cell && !meshNames.has(name)) this.unmesh(cell);
        }
        this.meshWanted = meshNames;
        // The mesh cells and their neighbours stay: their rims, or that they are empty, settle the meshes.
        const rings = mesh.flatMap(({ node, distance }) => [node, ...around(node)].map(other => ({ node: other, distance })));
        const wantedNames = new Set([...cells, ...rings].map(({ node }) => key(node)));
        for (const [name, cell] of this.cells) if (!wantedNames.has(name) && cell.state !== 'built' && cell.state !== 'asked') this.cells.delete(name);
        const built = [...this.cells.values()].flatMap(c => (c.state === 'built' ? [c.rows] : []));
        const guess = built.length ? Math.ceil(built.reduce((a, b) => a + b, 0) / built.length) : GUESS_ROWS;
        this.target.clear();
        let rows = 0;
        this.wantedM = cells.length ? cells[cells.length - 1].distance : 0;
        const asks: { node: Node; distance: number }[] = [];
        // A mesh cell built without its input builds again first.
        for (const name of meshNames) {
            const cell = this.cells.get(name);
            if (cell?.state === 'built' && cell.job === undefined && cell.worker === undefined && cell.mesh === undefined) asks.push(cell);
        }
        const later = rings.filter(({ node }) => !this.cells.has(key(node)));
        for (const { node, distance } of cells) {
            const name = key(node), cell = this.cells.get(name);
            if (cell) cell.distance = distance;
            if (cell?.state === 'empty' || cell?.state === 'failed') continue;
            rows += (cell && 'rows' in cell ? cell.rows : undefined) ?? guess;
            if (rows > ROWS - 1) break;
            this.target.add(name);
            if (!cell || (cell.state === 'over' && cell.job === undefined && this.room(cell.rows, this.leaving(cell)))) later.push({ node, distance });
        }
        const asked = new Set<string>();
        for (const { node, distance } of [...asks, ...later.sort((a, b) => a.distance - b.distance)]) {
            if (asked.has(key(node))) continue;
            asked.add(key(node));
            if (!this.ask(node, distance)) break;
        }
        for (const name of meshNames) this.meshIfReady(name);
    }

    /** Asks a free worker for a cell, with its mesh input kept when the mesh wants it; false when no worker is free. */
    private ask(node: Node, distance: number): boolean {
        const k = this.pool.findIndex(slot => slot?.ready && [...slot.jobs.values()].length < JOBS);
        if (k < 0) return false;
        const name = key(node), slot = this.pool[k], id = this.next++, keep = this.meshWanted.has(name);
        const old = this.cells.get(name);
        // A built cell keeps its block while it builds again for its mesh; an `over` cell keeps its size.
        const cell: Cell = old?.state === 'built' ? old : { node, distance, state: 'asked', rows: old?.state === 'over' ? old.rows : undefined };
        Object.assign(cell, { job: id, keep });
        this.cells.set(name, cell);
        slot.jobs.set(id, name);
        void (async () => {
            try {
                const inputs = await cellInputs(node, { feature_tiles, wanted, words: this.words }, this.features!, this.stored);
                if (!slot.jobs.has(id) || slot.worker !== this.pool[k].worker) return;
                if (inputs === 'empty') {
                    slot.jobs.delete(id);
                    cell.job = undefined;
                    if (cell.state === 'asked') Object.assign(cell, { state: 'empty' });
                    this.onChange();
                    return;
                }
                const message: PlanRequest = { type: 'cell', id, cell: node, ...inputs, mesh: keep };
                slot.worker.postMessage(message, inputs.tiles.map(({ body }) => body));
            } catch (error) {
                slot.jobs.delete(id);
                if (stopped(error)) return;
                cell.job = undefined;
                if (cell.state === 'asked') Object.assign(cell, { state: 'failed' });
                else cell.mesh = 'done';
                this.onChange();
            }
        })();
        return true;
    }

    /** Asks for a cell's mesh once it is built with its input kept and each neighbour is built or empty. */
    private meshIfReady(name: string): void {
        const cell = this.cells.get(name);
        if (cell?.state !== 'built' || cell.worker === undefined || cell.mesh !== undefined || !cell.rim) return;
        const rims: Uint32Array[] = [cell.rim];
        let waiting = false;
        for (const other of around(cell.node)) {
            const neighbour = this.cells.get(key(other));
            if (neighbour?.rim) rims.push(neighbour.rim);
            else if (neighbour?.state !== 'empty' && neighbour?.state !== 'failed') {
                // A neighbour on its way, or not asked yet: it builds for its rim.
                if (!neighbour) this.ask(other, cell.distance);
                waiting = true;
            }
        }
        if (waiting) return;
        const slot = this.pool[cell.worker], id = this.next++;
        const all = new Uint32Array(rims.reduce((sum, r) => sum + r.length, 0));
        rims.reduce((at, r) => (all.set(r, at), at + r.length), 0);
        cell.mesh = 'asked';
        cell.meshJob = id;
        slot.jobs.set(id, name);
        slot.worker.postMessage({ type: 'mesh', id, cell: cell.node, rims: all } satisfies PlanRequest, [all.buffer]);
    }

    /** Whether a page of a near node waits for its cell's block: its cell is in the target and not built yet. Nothing waits once the records failed. */
    waits(node: Node): boolean {
        if (node.level < CELL_LEVEL || this.error) return false;
        const name = key(cellOf(node)), state = this.cells.get(name)?.state;
        return this.target.has(name) && (state === undefined || state === 'asked');
    }

    /** Whether a cell's mesh is still to come: the mesh wants the cell, and its build and its mesh can still succeed. */
    meshComing(node: Node): boolean {
        const name = key(node), cell = this.cells.get(name);
        return !this.error && this.meshWanted.has(name) && !(cell && (cell.state === 'empty' || cell.state === 'failed' || cell.state === 'over' || cell.mesh === 'done'));
    }

    /** The rows of each built cell's block, for the determinism check. */
    blocks(): { name: string; first: number; rows: number }[] {
        return [...this.cells].flatMap(([name, cell]) => (cell.state === 'built' ? [{ name, first: cell.first, rows: cell.rows }] : []));
    }

    /** The first word of a cell's block in the texture, undefined while it has none. */
    block(node: Node): number | undefined {
        const cell = this.cells.get(key(node));
        return cell?.state === 'built' ? cell.first * MODEL_ROW : undefined;
    }

    /** Whether a cell is settled for the mesh: meshed, or with no mesh to come. */
    settled(node: Node): boolean {
        const cell = this.cells.get(key(node));
        return !!cell && (cell.state === 'empty' || cell.state === 'failed' || cell.state === 'over' || cell.mesh === 'done');
    }

    /** The cells whose block arrived or left since the last call: their pages are stamped again. */
    takeChanged(): Node[] {
        const out = this.changed;
        this.changed = [];
        return out;
    }

    /** Uploads the arrived blocks and the table of the cells about `eye` (a world point of level 30). */
    write(device: GPUDevice, texture: GPUTexture, eye: [number, number]): void {
        for (const { first, words } of this.uploads) device.queue.writeTexture({ texture, origin: [0, first] }, words, { bytesPerRow: MODEL_ROW * 4 }, [MODEL_ROW, words.length / MODEL_ROW]);
        this.uploads = [];
        const origin: [number, number] = [Math.floor(eye[0] / 2 ** 14) - TABLE / 2, Math.floor(eye[1] / 2 ** 14) - TABLE / 2];
        if (!this.tableDirty && origin[0] === this.origin[0] && origin[1] === this.origin[1]) return;
        this.origin = origin;
        this.tableDirty = false;
        const table = new Uint32Array(MODEL_ROW);
        table.set([TABLE_MAGIC, origin[0], origin[1], TABLE, TABLE]);
        for (const cell of this.cells.values()) {
            const i = cell.node.x - origin[0], j = cell.node.y - origin[1];
            if (cell.state === 'built' && i >= 0 && j >= 0 && i < TABLE && j < TABLE) table[5 + j * TABLE + i] = cell.first * MODEL_ROW;
        }
        device.queue.writeTexture({ texture }, table, { bytesPerRow: MODEL_ROW * 4 }, [MODEL_ROW, 1]);
    }

    /** A new device holds no block: every cell builds again. */
    lost(): void {
        for (const [name, cell] of this.cells) if (cell.state === 'built') this.drop(name, cell);
        this.changed = [];
        this.uploads = [];
        this.tableDirty = true;
    }

    /** True while a target cell has no block, or a mesh cell is not settled. False once the records failed, so a wait for an idle view ends. */
    get loading(): boolean {
        if (this.error) return false;
        return [...this.target].some(name => this.cells.get(name)?.state === 'asked' || !this.cells.has(name))
            || [...this.meshWanted].some(name => !this.settled(this.cells.get(name)?.node ?? nodeOf(name)));
    }

    get stats(): RecordsStats {
        const built = [...this.cells.values()].flatMap(c => (c.state === 'built' ? [c] : []));
        const median = (values: number[]) => [...values].sort((a, b) => a - b)[Math.floor(values.length / 2)] ?? 0;
        return {
            cells: built.length, bytes: built.reduce((sum, c) => sum + c.rows * MODEL_ROW * 4, 0), radius: Math.max(0, ...built.map(c => c.distance)), wanted: this.wantedM,
            building: this.pool.reduce((sum, slot) => sum + (slot?.jobs.size ?? 0), 0), build: median(this.times), mesh: median(this.meshTimes), restarts: this.restarts, heap: this.heap,
            ...(this.error ? { error: this.error } : {}),
        };
    }
}

const nodeOf = (name: string): Node => {
    const [level, x, y] = name.split('/').map(Number);
    return { level, x, y };
};

/** The cells under near nodes, each once, nearest first, with the metres from the eye to their squares. */
export function nearCells(nodes: Node[], eye: Parameters<typeof distances>[0]): { node: Node; distance: number }[] {
    const cells = new Map<string, Node>();
    for (const node of nodes) if (node.level >= CELL_LEVEL) cells.set(key(cellOf(node)), cellOf(node));
    return [...cells.values()].map(node => ({ node, distance: distances(eye, node)[0] })).sort((a, b) => a.distance - b.distance);
}
