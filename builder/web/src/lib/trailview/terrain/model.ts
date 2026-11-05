// The records of the cells about the eye on the device (`ground=plan`): built in a worker from the
// cells' inputs, kept nearest first within RECORDS_MB, in one texture of MODEL_ROW words per row that
// the stamp pass reads (model.wgsl). Row 0 holds a table of the cells about the eye; each block takes
// whole rows after it.
import { stopped } from '../stream/terrain';
import { cellInputs, type StoredTiles } from '../plan/inputs';
import init, { feature_tiles, vocabulary, wanted } from '../plan/pkg/obc_ground_plan_bridge.js';
import { distances, key, type Node } from './lod';
import type { FeatureTiles } from './stamps';
import type { RecordsReply, RecordsRequest } from './model-worker';

const WASM = new URL('../plan/pkg/obc_ground_plan_bridge_bg.wasm', import.meta.url);
/** Words per row of the texture: MODEL_ROW of records.wgsl. */
export const MODEL_ROW = 4096;
/** The level of the nodes that are cells: zoom 16, NEAR of bins.ts. */
export const CELL_LEVEL = 16;
/**
 * The records' GPU memory. The cells under the wanted near pages come first, nearest first; a town
 * shrinks how far they reach.
 */
export const RECORDS_MB = 24;
export const ROWS = (RECORDS_MB * 2 ** 20) / (MODEL_ROW * 4);
/** The table's first word, as M_TABLE of model.wgsl, and its cells per side about the eye. */
const TABLE_MAGIC = 0x4f42540a;
const TABLE = 48;
/** Builds that the worker holds: one runs and one waits. */
const JOBS = 2;
/** Rows that a block not built yet counts as, until blocks are known. */
const GUESS_ROWS = 16;

type Cell = { node: Node; distance: number } & ({ state: 'asked' } | { state: 'built'; first: number; rows: number } | { state: 'empty' | 'failed' | 'over' });

export interface RecordsStats {
    /** Cells with a block, and their bytes on the device. */
    cells: number;
    bytes: number;
    /** Metres from the eye to the farthest cell with a block, and to the farthest wanted cell. */
    radius: number;
    wanted: number;
    building: number;
    /** The median build of a cell's records in the worker, in ms. */
    build: number;
    error?: string;
}

/** The cell of a node of level CELL_LEVEL or finer. */
export const cellOf = ({ level, x, y }: Node): Node => ({ level: CELL_LEVEL, x: x >> (level - CELL_LEVEL), y: y >> (level - CELL_LEVEL) });

export class Records {
    private cells = new Map<string, Cell>();
    /** Free row ranges [first, count]; row 0 is the table. */
    private free: [number, number][] = [[1, ROWS - 1]];
    private uploads: { first: number; words: Uint32Array<ArrayBuffer> }[] = [];
    /** The cells whose block arrived or left since the last `takeChanged`. */
    private changed: Node[] = [];
    private target = new Set<string>();
    private origin: [number, number] = [-1, -1];
    private tableDirty = true;
    private worker?: Worker;
    private module?: WebAssembly.Module;
    private ready = false;
    private words = 0;
    private jobs = new Map<number, string>();
    private next = 0;
    private times: number[] = [];
    private error?: string;
    private wantedM = 0;

    constructor(private readonly features: FeatureTiles | undefined, private readonly stored: StoredTiles, private readonly onChange: () => void, private readonly stop: AbortSignal) {
        stop.addEventListener('abort', () => this.worker?.terminate());
        if (features) void this.load();
    }

    private async load(): Promise<void> {
        try {
            const module = await WebAssembly.compile(await (await fetch(WASM, { signal: this.stop })).arrayBuffer());
            await init({ module_or_path: module });
            this.words = JSON.parse(vocabulary()).words.wanted;
            this.module = module;
            this.spawn();
        } catch (error) {
            if (stopped(error)) return;
            this.error = `the records bridge did not start: ${error instanceof Error ? error.message : error}`;
            this.onChange();
        }
    }

    private spawn(): void {
        if (this.stop.aborted) return;
        const worker = this.worker = new Worker(new URL('./model-worker.ts', import.meta.url), { type: 'module' });
        this.ready = false;
        worker.onmessage = ({ data }: MessageEvent<RecordsReply>) => this.receive(worker, data);
        worker.postMessage({ type: 'start', module: this.module! } satisfies RecordsRequest);
    }

    private receive(worker: Worker, data: RecordsReply): void {
        if (worker !== this.worker) return;
        if ('type' in data) {
            if (data.error) this.error = `the records worker did not start: ${data.error}`;
            else this.ready = true;
            return this.onChange();
        }
        const name = this.jobs.get(data.id);
        this.jobs.delete(data.id);
        const cell = name === undefined ? undefined : this.cells.get(name);
        if ('error' in data) {
            console.warn(`Records of ${name}: ${data.error}`);
            if (data.trapped) {
                this.worker.terminate();
                for (const other of this.jobs.values()) this.cells.delete(other);
                this.jobs.clear();
                this.spawn();
            }
            if (cell) cell.state = 'failed';
        } else if (cell?.state === 'asked') {
            this.times = [...this.times.slice(-31), data.ms];
            this.place(cell, data.words);
        }
        this.onChange();
    }

    /** Puts a block into free rows, making room from the cells past the target, farthest first. */
    private place(cell: Cell, words: Uint32Array): void {
        const rows = Math.ceil(words.length / MODEL_ROW);
        let first = this.allocate(rows);
        const leaving = [...this.cells.entries()].filter(([other, c]) => c.state === 'built' && !this.target.has(other)).sort((a, b) => b[1].distance - a[1].distance);
        while (first === undefined && leaving.length) {
            this.drop(...leaving.shift()!);
            first = this.allocate(rows);
        }
        if (first === undefined) {
            Object.assign(cell, { state: 'over' });
            return;
        }
        Object.assign(cell, { state: 'built', first, rows });
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
            this.free.push([cell.first, cell.rows]);
            this.free.sort((a, b) => a[0] - b[0]);
            this.free = this.free.reduce<[number, number][]>((out, range) => {
                const last = out[out.length - 1];
                if (last && last[0] + last[1] === range[0]) last[1] += range[1];
                else out.push([...range]);
                return out;
            }, []);
            this.changed.push(cell.node);
            this.tableDirty = true;
        }
        this.cells.delete(name);
    }

    /**
     * Takes the cells under the frame's wanted near pages, nearest first: those whose blocks fit the
     * budget are the target, and the worker builds the next of them while `build` lets it.
     */
    update(cells: { node: Node; distance: number }[], build: boolean): void {
        const wantedNames = new Set(cells.map(({ node }) => key(node)));
        for (const [name, cell] of this.cells) if (!wantedNames.has(name) && cell.state !== 'built' && cell.state !== 'asked') this.cells.delete(name);
        const built = [...this.cells.values()].flatMap(c => (c.state === 'built' ? [c.rows] : []));
        const guess = built.length ? Math.ceil(built.reduce((a, b) => a + b, 0) / built.length) : GUESS_ROWS;
        this.target.clear();
        let rows = 0;
        this.wantedM = cells.length ? cells[cells.length - 1].distance : 0;
        for (const { node, distance } of cells) {
            const name = key(node), cell = this.cells.get(name);
            if (cell) cell.distance = distance;
            if (cell?.state === 'empty' || cell?.state === 'failed' || cell?.state === 'over') continue;
            rows += cell?.state === 'built' ? cell.rows : guess;
            if (rows > ROWS - 1) break;
            this.target.add(name);
            if (!cell && build && this.ready && this.jobs.size < JOBS) void this.ask(node, distance);
        }
    }

    private async ask(node: Node, distance: number): Promise<void> {
        const name = key(node), cell: Cell = { node, distance, state: 'asked' };
        this.cells.set(name, cell);
        const id = this.next++;
        this.jobs.set(id, name);
        try {
            const inputs = await cellInputs(node, { feature_tiles, wanted, words: this.words }, this.features!, this.stored);
            if (this.cells.get(name) !== cell || !this.jobs.has(id)) return;
            if (inputs === 'empty') {
                this.jobs.delete(id);
                Object.assign(cell, { state: 'empty' });
                this.onChange();
                return;
            }
            const message: RecordsRequest = { type: 'build', id, cell: node, ...inputs, words: this.words };
            this.worker!.postMessage(message, inputs.tiles.map(({ body }) => body));
        } catch (error) {
            this.jobs.delete(id);
            if (stopped(error)) return;
            Object.assign(cell, { state: 'failed' });
            this.onChange();
        }
    }

    /** Whether a page of a near node waits for its cell's block: its cell is in the target and not built yet. */
    waits(node: Node): boolean {
        if (node.level < CELL_LEVEL) return false;
        const name = key(cellOf(node)), state = this.cells.get(name)?.state;
        return this.target.has(name) && (state === undefined || state === 'asked');
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

    /** True while a target cell has no block. */
    get loading(): boolean {
        return !this.error && !!this.features && [...this.target].some(name => this.cells.get(name)?.state === 'asked' || !this.cells.has(name));
    }

    get stats(): RecordsStats {
        const built = [...this.cells.values()].flatMap(c => (c.state === 'built' ? [c] : []));
        const sorted = [...this.times].sort((a, b) => a - b);
        return {
            cells: built.length, bytes: built.reduce((sum, c) => sum + c.rows * MODEL_ROW * 4, 0), radius: Math.max(0, ...built.map(c => c.distance)), wanted: this.wantedM,
            building: this.jobs.size, build: sorted[Math.floor(sorted.length / 2)] ?? 0, ...(this.error ? { error: this.error } : {}),
        };
    }
}

/** The cells under near nodes, each once, nearest first, with the metres from the eye to their squares. */
export function nearCells(nodes: Node[], eye: Parameters<typeof distances>[0]): { node: Node; distance: number }[] {
    const cells = new Map<string, Node>();
    for (const node of nodes) if (node.level >= CELL_LEVEL) cells.set(key(cellOf(node)), cellOf(node));
    return [...cells.values()].map(node => ({ node, distance: distances(eye, node)[0] })).sort((a, b) => a.distance - b.distance);
}
