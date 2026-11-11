// The records of the cells about the eye on the device (`ground=plan`): built in plan workers from the
// cells' inputs, kept nearest first within RECORDS_MB, in one texture of MODEL_ROW words per row that
// the stamp pass reads (model.wgsl). Row 0 holds a table of the cells about the eye; each block takes
// whole rows after it. One rank orders the rows: the block of a nearer cell is better, by the cell's own
// distance to the eye. A cell builds only for rows that are free for it, or for its rim, which the mesh
// of a neighbour with rows reads. The same build gives a cell's rim and, for the cells that the mesh
// wants, keeps the input of its mesh in its worker, which meshes it once the rims of its eight neighbours
// are here: each cell builds once. With a cache key, built cells and meshes go to the device cache
// (device-cache.ts), which a cell reads before it collects its inputs. A worker's answer asks for the next
// cells at once, also between frames.
import { stopped } from '../stream/terrain';
import { CAPS, DeviceCache } from './device-cache';
import { bridge, planWorker } from '../plan/bridge';
import { cellInputs, type CellInputs, type StoredTiles } from '../plan/inputs';
import { HEAP_LIMIT } from '../plan/plan';
import { feature_tiles, wanted } from '../plan/pkg/obc_ground_plan_bridge.js';
import type { CellBuilt, Failed, Meshed, Missing, PlanRequest } from '../plan/plan-worker';
import { distances, key, type Node } from './lod';
import type { FeatureTiles } from './stamps';

/** Words per row of the texture: MODEL_ROW of records.wgsl. */
export const MODEL_ROW = 4096;
/** The level of the nodes that are cells: zoom 16, NEAR of bins.ts. */
export const CELL_LEVEL = 16;
/**
 * The records' GPU memory. The nearest cells hold it; a town shrinks how far they reach. It holds
 * every wanted cell of a town view of the fixture.
 */
export const RECORDS_MB = 32;
export const ROWS = (RECORDS_MB * 2 ** 20) / (MODEL_ROW * 4);
/** The table's first word, as M_TABLE of model.wgsl, and its cells per side about the eye. */
const TABLE_MAGIC = 0x4f42540a;
const TABLE = 48;
/**
 * Plan workers, and the builds asked for per worker: one runs, and one waits with its inputs for the next free worker.
 * Two workers halve the time to a full view after a jump.
 */
export const WORKERS = 2;
const JOBS = 2;
/**
 * The rows that a build for rows starts with: free for its block, after the blocks of the nearer cells.
 * A larger block (`large`), and the block of a cell that is tried with fewer free rows, can find no rows.
 */
export const RESERVE_ROWS = 384;

type Ranked = { node: Node; distance: number };

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
    /** A kept input was lost: the mesh still comes, but the cell's pages no longer wait for it. */
    lost?: boolean;
    /** The labels of its block that are junctions with a stop arm (mouthLabels). */
    mouths?: Set<number>;
    /** The mesh asked for lacks the rim of a failed neighbour: it differs from a built one, so it is not cached. */
    partial?: boolean;
    /** The crate refused the far block of its last build. */
    farFailed?: boolean;
} & (
    // `rows` is the block's size once known. An `over` cell's block has no rows: it asks again when the rank gives it rows.
    // It holds `words`, its block, while a nearer block on its way decides whether it fits.
    | { state: 'asked'; rows?: number }
    | { state: 'built'; first: number; rows: number }
    | { state: 'over'; rows: number; words?: Uint32Array }
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
    /** The cells read from the device cache, and the bytes that it holds. */
    hits: number;
    cacheBytes: number;
    restarts: number;
    /** The largest heap of a worker, in bytes. */
    heap: number;
    /** The cells whose build failed or whose far block the crate refused. */
    failed: number;
    /**
     * The builds that the workers answered; the blocks of ranked cells that found no rows and whose rim no mesh reads;
     * each arrival of a block over RESERVE_ROWS, from a worker or the cache and again for each build of the same cell:
     * a build for such a block can end without rows.
     */
    builds: number;
    refused: number;
    large: number;
    error?: string;
}

/** Row ranges [first, count], sorted, with touching ranges joined. */
const merged = (ranges: [number, number][]) => [...ranges].sort((a, b) => a[0] - b[0]).reduce<[number, number][]>((out, range) => {
    const last = out[out.length - 1];
    if (last && last[0] + last[1] === range[0]) last[1] += range[1];
    else out.push([...range]);
    return out;
}, []);

/** A cell's mesh (`mesh/mod.rs` of obc-ground-plan gives the bytes) and the first word of its block in the texture. */
export interface CellMeshBytes { node: Node; bytes: ArrayBuffer; block: number; mouths: Set<number> }

/** As LABEL, the junction owner, the junction record (JR, JR_ARM) and ARM_STOP of the crate's `faces::records`. */
const LABEL_WORDS = 12, OWNER_JUNCTION = 2, JR = 3, JR_ARM = 3, ARM_STOP = 1 << 17;

/** The labels of a records block that are junctions with a stop arm: their faces draw with the mouth's look (plan/pieces.ts). */
export function mouthLabels(words: Uint32Array): Set<number> {
    const out = new Set<number>(), first = words[5];
    for (let id = 0; id < words[2]; id++) {
        const l = first + id * LABEL_WORDS;
        if (((words[l] >>> 8) & 0xff) !== OWNER_JUNCTION) continue;
        const jr = words[words[12] + words[l + 1]];
        for (let k = 0; k < (words[jr + 2] & 0xff); k++) if (words[jr + JR + JR_ARM * k] & ARM_STOP) out.add(id);
    }
    return out;
}

/** The cell of a node of level CELL_LEVEL or finer. */
export const cellOf = ({ level, x, y }: Node): Node => ({ level: CELL_LEVEL, x: x >> (level - CELL_LEVEL), y: y >> (level - CELL_LEVEL) });

const around = ({ x, y }: Node): Node[] => [-1, 0, 1].flatMap(dy => [-1, 0, 1].flatMap(dx => (dx || dy ? [{ level: CELL_LEVEL, x: x + dx, y: y + dy }] : [])));

/**
 * A plan worker, its jobs by id, the build among them, its heap after its last build, the feature tiles that it parsed
 * (as column/row), and per job on its way the tiles whose body it carries: a tile counts as parsed once that job
 * answers with its build.
 */
interface Slot { worker: Worker; ready: boolean; jobs: Map<number, string>; build?: number; heap: number; tiles: Set<string>; carried: Map<number, string[]> }

/** A build asked for: it waits for its inputs, then for a free worker. */
interface Job { id: number; name: string; cell: Cell; inputs?: CellInputs }

export class Records {
    private cells = new Map<string, Cell>();
    /** Free row ranges [first, count]; row 0 is the table. */
    private free: [number, number][] = [[1, ROWS - 1]];
    private uploads: { first: number; words: Uint32Array<ArrayBuffer> }[] = [];
    /** The cells whose block arrived or left since the last `takeChanged`. */
    private changed: Node[] = [];
    /** The cells that want rows, nearest first, with their own metres from the eye. */
    private rank = new Map<string, Ranked>();
    /** The cells of the rank that get rows, and those that can still get rows once the nearer blocks on their way are known. */
    private target = new Set<string>();
    private open = new Set<string>();
    private meshWanted = new Set<string>();
    private origin: [number, number] = [-1, -1];
    private tableDirty = true;
    private pool: Slot[] = [];
    /** The builds not posted yet, in the order of their asks. */
    private asked: Job[] = [];
    private module?: WebAssembly.Module;
    private words = 0;
    private next = 0;
    private times: number[] = [];
    private meshTimes: number[] = [];
    private restarts = 0;
    private builds = 0;
    private refused = 0;
    private large = 0;
    private error?: string;
    private wantedM = 0;
    private heap = 0;
    private cache?: Promise<DeviceCache | undefined>;
    /** The cache once open, for the stats. */
    private opened?: DeviceCache;
    private hits = 0;
    /** The arguments of the last `update`, which every answer of a worker asks again with. */
    private last?: Parameters<Records['update']>;

    /**
     * `meshed` takes each cell's mesh as it arrives, `unmeshed` each cell whose mesh leaves. `cache` names the
     * data of the tiles and the version of the plan worker's input preparation (`virtual:plan-version`): without
     * it, nothing is cached.
     */
    constructor(private readonly features: FeatureTiles | undefined, private readonly stored: StoredTiles, private readonly onChange: () => void, private readonly stop: AbortSignal,
        private readonly meshed: (mesh: CellMeshBytes) => void = () => {}, private readonly unmeshed: (node: Node) => void = () => {}, private readonly cacheKey?: { release: string; version: string }) {
        stop.addEventListener('abort', () => this.pool.forEach(({ worker }) => worker.terminate()));
        if (!features) {
            this.error = 'the release has no feature tiles';
            return;
        }
        void this.load();
    }

    private async load(): Promise<void> {
        try {
            const { module, vocabulary, digest } = await bridge();
            if (this.cacheKey) {
                const { release, version } = this.cacheKey;
                this.cache = DeviceCache.open('cells', release, `${digest}/${version}`, CAPS.cells).then(cache => (this.opened = cache), error => {
                    console.warn(`The ground plan's device cache did not open: ${error}`);
                    return undefined;
                });
            }
            this.words = vocabulary.words.wanted;
            this.module = module;
            for (let k = 0; k < WORKERS; k++) this.spawn(k);
        } catch (error) {
            this.error = `the records bridge did not start: ${error instanceof Error ? error.message : error}`;
            this.onChange();
        }
    }

    private spawn(k: number): void {
        if (this.stop.aborted) return;
        const worker = planWorker();
        this.pool[k] = { worker, ready: false, jobs: new Map(), heap: 0, tiles: new Set(), carried: new Map() };
        worker.onmessage = ({ data }: MessageEvent<{ type: 'ready'; error?: string } | CellBuilt | Meshed | Missing | Failed>) => this.receive(k, worker, data);
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
        for (const cell of this.cells.values()) {
            if (cell.worker !== k) continue;
            cell.worker = undefined;
            cell.lost = true;
        }
        this.spawn(k);
    }

    /**
     * Ends worker k once its heap is full, no build waits, it is idle, and it keeps no input of a mesh still to ask:
     * WASM memory never shrinks, and a restart that drops a kept input makes the cell build again.
     */
    private recycle(k: number): void {
        const slot = this.pool[k];
        if (slot.heap > HEAP_LIMIT && !slot.jobs.size && !this.asked.length && ![...this.cells.values()].some(cell => cell.worker === k)) this.restart(k);
    }

    private receive(k: number, worker: Worker, data: { type: 'ready'; error?: string } | CellBuilt | Meshed | Missing | Failed): void {
        const slot = this.pool[k];
        if (worker !== slot.worker) return;
        if ('type' in data) {
            if (data.error) this.error = `the records worker did not start: ${data.error}`;
            else slot.ready = true;
            return this.answered();
        }
        const carried = slot.carried.get(data.id) ?? [];
        slot.carried.delete(data.id);
        if ('missing' in data) {
            for (let i = 0; i < data.missing.length; i += 2) slot.tiles.delete(`${data.missing[i]}/${data.missing[i + 1]}`);
            const name = slot.jobs.get(data.id) ?? '', cell = this.cells.get(name);
            // The job goes again with every body, so it cannot miss twice. The job of a cell that left the rank ends.
            if (cell?.job === data.id && this.rank.has(name)) void this.resend(k, data.id, cell);
            else {
                if (cell?.job === data.id) cell.job = undefined;
                slot.jobs.delete(data.id);
                slot.build = undefined;
                this.answered();
            }
            return;
        }
        if (slot.build === data.id) slot.build = undefined;
        if ('words' in data) carried.forEach(tile => slot.tiles.add(tile));
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
            slot.heap = data.heap;
            if (cell?.job === data.id) {
                cell.farFailed = 'error' in data.far;
                if ('error' in data.far) console.warn(`Ground plan cell ${name}: far block: ${data.far.error}`);
                this.times = [...this.times.slice(-31), data.ms];
                this.builds++;
                this.keep(cell, 'cell', [data.words, data.rim]);
                if (cell.keep && this.meshWanted.has(name!)) cell.worker = k;
                else if (cell.keep) worker.postMessage({ type: 'drop', cell: cell.node } satisfies PlanRequest);
                this.built(cell, data.words, data.rim);
            }
        } else if (cell?.mesh === 'asked' && cell.meshJob === data.id) {
            cell.worker = undefined;
            if ('gone' in data) Object.assign(cell, { mesh: undefined, lost: true });
            else {
                if (!cell.partial) this.keep(cell, 'mesh', [new Uint32Array(data.mesh)]);
                cell.mesh = undefined;
                this.meshTimes = [...this.meshTimes.slice(-31), data.ms];
                this.mesh(cell, data.mesh);
            }
        }
        this.answered();
    }

    /** After an answer of a worker or the cache: the next cells are asked for at once, and the next frame draws what came. */
    private answered(): void {
        if (this.last && !this.stop.aborted) this.update(...this.last);
        this.onChange();
    }

    /** Takes a cell's block and rim, from its worker or the cache. A built cell that built again for its mesh keeps its block. */
    private built(cell: Cell, words: Uint32Array, rim: Uint32Array): void {
        cell.job = undefined;
        cell.rim = rim;
        if (cell.state === 'built') return;
        const rows = Math.ceil(words.length / MODEL_ROW);
        if (rows > RESERVE_ROWS) this.large++;
        Object.assign(cell, { state: 'over', rows, words });
        this.decide();
    }

    /**
     * The cells of the rank whose blocks fit the rows, nearest first, up to the first that does not fit: so no cell is
     * refused while a farther cell gets rows. A known block takes its rows. A block not built yet needs RESERVE_ROWS,
     * which it takes from the cells after it if `sure`. Where the reserve does not fit, the rows fill one cell at a
     * time: a cell is tried while the mean of the nearer blocks fits and, if `sure`, only once each nearer block is
     * known, with no cell after it. A block larger than the texture takes no part.
     */
    private fitting(sure: boolean): Set<string> {
        const out = new Set<string>();
        let left = ROWS - 1, known = 0, count = 0, coming = 0;
        for (const name of this.rank.keys()) {
            const cell = this.cells.get(name);
            if (cell?.state === 'empty' || cell?.state === 'failed') continue;
            const rows = cell && 'rows' in cell ? cell.rows : undefined;
            if (rows !== undefined) {
                if (rows > ROWS - 1) continue;
                if (rows > left) break;
                left -= rows;
                known += rows;
                count++;
            } else if (RESERVE_ROWS <= left) {
                if (sure) left -= RESERVE_ROWS;
                coming++;
            } else if ((sure && coming) || !count || left < known / count) break;
            else if (sure) {
                out.add(name);
                break;
            }
            out.add(name);
        }
        return out;
    }

    /**
     * Makes the target: the cells that get rows also if each block on its way takes RESERVE_ROWS. A block that arrived
     * takes its rows when its cell is in the target. It waits while its cell is `open`: in the target if the nearer
     * blocks on their way are small. Else it goes: its cell keeps the rim and the size.
     */
    private decide(): void {
        this.target = this.fitting(true);
        this.open = this.fitting(false);
        for (const [name, cell] of this.cells) {
            if (cell.state !== 'over' || !cell.words) continue;
            if (this.target.has(name)) this.place(cell, cell.words);
            else if (!this.open.has(name)) this.refuse(name, cell);
        }
    }

    /** Lets the block of an `over` cell go, with the kept input of its mesh. */
    private refuse(name: string, cell: Cell): void {
        if (this.rank.has(name) && !around(cell.node).some(other => this.reads(key(other)))) this.refused++;
        Object.assign(cell, { words: undefined });
        this.unmesh(cell);
    }

    /** Whether a cell's mesh reads the rims of its neighbours: the mesh wants it, and it has rows or its build gets them. */
    private reads(name: string): boolean {
        const state = this.cells.get(name)?.state;
        return this.meshWanted.has(name) && (state === 'built' || (this.target.has(name) && (state === undefined || state === 'asked')));
    }

    /** Draws a cell's mesh while the cell has its block and the mesh wants it. */
    private mesh(cell: Cell, bytes: ArrayBuffer): void {
        if (cell.state !== 'built' || !this.meshWanted.has(key(cell.node))) return;
        cell.mesh = 'done';
        this.meshed({ node: cell.node, bytes, block: cell.first * MODEL_ROW, mouths: cell.mouths ?? new Set() });
    }

    /** Writes a cell's entry to the device cache, off the critical path. */
    private keep(cell: Cell, part: 'cell' | 'mesh', parts: Uint32Array[]): void {
        void this.cache?.then(cache => cache?.write(`${cell.node.x}/${cell.node.y}/${part}`, parts)).catch(error => console.warn(`The ground plan's device cache did not keep ${key(cell.node)}: ${error}`));
    }

    /** A cell from the device cache: its block and rim, and its mesh when `mesh` and the cache holds it; undefined without the cell's entry. */
    private async cached(node: Node, mesh: boolean): Promise<{ words: Uint32Array; rim: Uint32Array; mesh?: ArrayBuffer } | undefined> {
        const cache = await this.cache;
        if (!cache) return undefined;
        try {
            const [cell, meshed] = await Promise.all([cache.read(`${node.x}/${node.y}/cell`), mesh ? cache.read(`${node.x}/${node.y}/mesh`) : undefined]);
            if (!cell) return undefined;
            return { words: cell[0], rim: cell[1], mesh: meshed?.[0].buffer as ArrayBuffer | undefined };
        } catch {
            return undefined;
        }
    }

    /** The built cells that leave for the block of a target cell, in order: those past the rank, then those past the target, each farthest first. */
    private leaving(): [string, Cell][] {
        return [...this.cells.entries()].filter(([name, c]) => c.state === 'built' && !this.target.has(name))
            .sort(([a, ca], [b, cb]) => Number(this.rank.has(a)) - Number(this.rank.has(b)) || cb.distance - ca.distance);
    }

    /** Whether a run of `rows` free rows exists once the `leaving` cells go. */
    private room(rows: number, leaving: [string, Cell][]): boolean {
        const taken = leaving.flatMap(([, c]): [number, number][] => (c.state === 'built' ? [[c.first, c.rows]] : []));
        return merged([...this.free, ...taken]).some(([, count]) => count >= rows);
    }

    /**
     * Puts the block of a target cell into free rows; the `leaving` cells go, in order, until it fits. The target's
     * blocks fit by their count of rows; where the free rows make no run of its size, the block goes.
     */
    private place(cell: Cell, words: Uint32Array): void {
        const rows = Math.ceil(words.length / MODEL_ROW), leaving = this.leaving();
        if (!this.room(rows, leaving)) return this.refuse(key(cell.node), cell);
        let first = this.allocate(rows);
        while (first === undefined) {
            this.evict(leaving.shift()![1]);
            first = this.allocate(rows);
        }
        Object.assign(cell, { state: 'built', first, rows, words: undefined, mouths: mouthLabels(words) });
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

    /** Takes a built cell's block off the rows, and its mesh off: the cell keeps its rim and its size. */
    private evict(cell: Cell): void {
        if (cell.state !== 'built') return;
        this.free = merged([...this.free, [cell.first, cell.rows]]);
        this.changed.push(cell.node);
        this.tableDirty = true;
        this.unmesh(cell);
        Object.assign(cell, { state: 'over' });
    }

    /** Takes a cell's mesh off, and frees its kept input. */
    private unmesh(cell: Cell): void {
        if (cell.mesh === 'done') this.unmeshed(cell.node);
        if (cell.worker !== undefined && cell.mesh !== 'asked') this.pool[cell.worker]?.worker.postMessage({ type: 'drop', cell: cell.node } satisfies PlanRequest);
        cell.mesh = undefined;
        cell.worker = undefined;
    }

    /**
     * Takes the cells under the frame's wanted near pages and the cells of `mesh`, which get their meshes, each with
     * its metres from the eye; `near` gives those of a neighbour of a mesh cell, which wants rows as they do. The
     * workers build the next cells of the target and, for each mesh cell that has rows or gets them, its neighbours
     * for their rims: in one near-first order, each neighbour as near as its mesh cell and, of two as near, the
     * nearer cell first. So the eye's cell comes first, then its eight neighbours. With `meshLater` (the meshes do
     * not draw) the neighbours build only after the whole target.
     */
    update(cells: Ranked[], mesh: Ranked[] = [], meshLater = false, near: (node: Node) => number = () => Infinity): void {
        this.last = [cells, mesh, meshLater, near];
        const meshNames = new Set(mesh.map(({ node }) => key(node)));
        for (const name of this.meshWanted) {
            const cell = this.cells.get(name);
            if (cell && !meshNames.has(name)) this.unmesh(cell);
        }
        this.meshWanted = meshNames;
        const ranked = new Map([...cells, ...mesh].map(entry => [key(entry.node), entry]));
        for (const { node } of mesh) for (const other of around(node)) if (!ranked.has(key(other))) ranked.set(key(other), { node: other, distance: near(other) });
        this.rank = new Map([...ranked].sort(([a, ca], [b, cb]) => ca.distance - cb.distance || (a < b ? -1 : 1)));
        // A build not posted yet whose cell left the rank is not built.
        this.asked = this.asked.filter(({ name, cell }) => {
            if (this.rank.has(name)) return true;
            cell.job = undefined;
            return false;
        });
        for (const [name, cell] of this.cells) {
            const own = this.rank.get(name);
            if (own) cell.distance = own.distance;
            // A cell past the rank keeps its block until a target cell takes its rows, and its posted build until it
            // answers: the answer drops the mesh input that the worker kept for it.
            else if (cell.state !== 'built' && cell.job === undefined) {
                this.unmesh(cell);
                this.cells.delete(name);
            }
        }
        this.wantedM = cells.length ? cells[cells.length - 1].distance : 0;
        this.decide();
        // A mesh cell built without its input builds again first.
        const again = mesh.filter(({ node }) => {
            const cell = this.cells.get(key(node));
            return cell?.state === 'built' && cell.job === undefined && cell.worker === undefined && cell.mesh === undefined;
        });
        // A target cell that lost its block asks again once its rows are free.
        const rows = [...this.target].flatMap(name => {
            const cell = this.cells.get(name);
            return !cell || (cell.state === 'over' && cell.job === undefined && this.room(cell.rows, this.leaving())) ? [this.rank.get(name)!] : [];
        });
        const rims = mesh.filter(({ node }) => this.reads(key(node))).flatMap(({ node, distance }) => around(node).filter(other => !this.cells.has(key(other))).map(other => ({ node: other, distance })));
        const own = (node: Node) => this.rank.get(key(node))?.distance ?? Infinity;
        const nearest = (list: Ranked[]) => list.sort((a, b) => a.distance - b.distance || own(a.node) - own(b.node));
        const order = meshLater ? [...nearest(rows), ...again, ...nearest(rims)] : [...again, ...nearest([...rims, ...rows])];
        const asked = new Set<string>();
        for (const { node, distance } of order) {
            if (asked.has(key(node))) continue;
            asked.add(key(node));
            if (!this.ask(node, distance)) break;
        }
        // A mesh goes to its worker before the next build.
        for (const name of meshNames) this.meshIfReady(name);
        this.dispatch();
        this.pool.forEach((_, k) => this.recycle(k));
    }

    /**
     * Asks for a cell, with its mesh input kept when the mesh wants it; false when the workers have their builds. The
     * cell comes from the device cache, or its build waits for its inputs and then for a free worker.
     */
    private ask(node: Node, distance: number): boolean {
        if (!this.module || this.asked.length + this.pool.filter(slot => slot.build !== undefined).length >= WORKERS * JOBS) return false;
        const name = key(node), id = this.next++, keep = this.meshWanted.has(name);
        const old = this.cells.get(name);
        // A built cell keeps its block while it builds again for its mesh; an `over` cell keeps its size and its rim.
        const cell: Cell = old?.state === 'built' ? old : old?.state === 'over' ? (Object.assign(old, { state: 'asked' }) as Cell) : { node, distance: this.rank.get(name)?.distance ?? distance, state: 'asked' };
        Object.assign(cell, { job: id, keep });
        this.cells.set(name, cell);
        const job: Job = { id, name, cell };
        this.asked.push(job);
        const leave = () => this.asked.includes(job) && this.asked.splice(this.asked.indexOf(job), 1).length > 0;
        void (async () => {
            try {
                const hit = await this.cached(node, keep);
                if (!this.asked.includes(job)) return;
                if (hit) {
                    this.hits++;
                    this.built(cell, hit.words, hit.rim);
                    if (hit.mesh) this.mesh(cell, hit.mesh);
                    // A placed block whose mesh the cache lacks builds again for the mesh's input; a block without rows has no mesh.
                    const done = hit.mesh || !keep || cell.state !== 'built';
                    if (done) leave();
                    else cell.job = id;
                    this.answered();
                    if (done) return;
                }
                // A feature tile that every worker parsed goes by name.
                const inputs = await cellInputs(node, { feature_tiles, wanted, words: this.words }, this.features!, this.stored, (column, row) => this.pool.every(slot => slot.tiles.has(`${column}/${row}`)));
                if (!this.asked.includes(job)) return;
                if (inputs === 'empty') {
                    leave();
                    return this.empty(cell);
                }
                job.inputs = inputs;
                this.dispatch();
            } catch (error) {
                if (leave()) this.lose(cell, error);
            }
        })();
        return true;
    }

    /**
     * Posts the builds whose inputs are here to the free workers, in the order of their asks: a worker stays free for
     * each earlier build whose inputs are on their way. So the eye's cell is the first build of its worker.
     */
    private dispatch(): void {
        const free = this.pool.flatMap((slot, k) => (slot.ready && slot.build === undefined ? [k] : []));
        for (let at = this.asked.findIndex(job => job.inputs); at >= 0 && at < free.length; at = this.asked.findIndex(job => job.inputs)) {
            const [job] = this.asked.splice(at, 1);
            this.post(free.shift()!, job.id, job.cell, job.inputs!);
        }
    }

    /** Posts a build to worker k: a feature tile that the worker parsed goes by name, unless `bodies`. */
    private post(k: number, id: number, cell: Cell, inputs: CellInputs, bodies = false): void {
        const slot = this.pool[k];
        const tiles = inputs.tiles.map(({ column, row, body }) => (!bodies && slot.tiles.has(`${column}/${row}`) ? { column, row } : { column, row, body }));
        slot.jobs.set(id, key(cell.node));
        slot.build = id;
        slot.worker.postMessage({ type: 'cell', id, cell: cell.node, ...inputs, tiles, mesh: !!cell.keep } satisfies PlanRequest, tiles.flatMap(({ body }) => (body ? [body] : [])));
        slot.carried.set(id, tiles.flatMap(({ column, row, body }) => (body ? [`${column}/${row}`] : [])));
    }

    /** Posts a build again with the body of every tile, after its worker missed one: so it cannot miss twice. */
    private async resend(k: number, id: number, cell: Cell): Promise<void> {
        const slot = this.pool[k];
        const leave = () => {
            slot.jobs.delete(id);
            if (slot.build === id) slot.build = undefined;
        };
        try {
            const inputs = await cellInputs(cell.node, { feature_tiles, wanted, words: this.words }, this.features!, this.stored);
            if (!slot.jobs.has(id) || slot !== this.pool[k]) return;
            if (inputs !== 'empty') return this.post(k, id, cell, inputs, true);
            leave();
            this.empty(cell);
        } catch (error) {
            leave();
            this.lose(cell, error);
        }
    }

    /** A cell without a feature tile has no block. */
    private empty(cell: Cell): void {
        cell.job = undefined;
        if (cell.state === 'asked') Object.assign(cell, { state: 'empty' });
        this.answered();
    }

    /** A build whose inputs did not come: its cell fails, unless the engine stopped. */
    private lose(cell: Cell, error: unknown): void {
        if (stopped(error)) return;
        cell.job = undefined;
        if (cell.state === 'asked') Object.assign(cell, { state: 'failed' });
        else cell.mesh = 'done';
        this.answered();
    }

    /** Asks for a cell's mesh once it is built with its input kept and each neighbour is built or empty. */
    private meshIfReady(name: string): void {
        const cell = this.cells.get(name);
        if (cell?.state !== 'built' || cell.worker === undefined || cell.mesh !== undefined || !cell.rim) return;
        const rims: Uint32Array[] = [cell.rim];
        let waiting = false, partial = false;
        for (const other of around(cell.node)) {
            const neighbour = this.cells.get(key(other));
            if (neighbour?.rim) rims.push(neighbour.rim);
            else if (neighbour?.state === 'failed') partial = true;
            // A neighbour on its way, or not asked yet: `update` asks for it, for its rim.
            else if (neighbour?.state !== 'empty') waiting = true;
        }
        if (waiting) return;
        const slot = this.pool[cell.worker], id = this.next++;
        const all = new Uint32Array(rims.reduce((sum, r) => sum + r.length, 0));
        rims.reduce((at, r) => (all.set(r, at), at + r.length), 0);
        Object.assign(cell, { mesh: 'asked', meshJob: id, partial });
        slot.jobs.set(id, name);
        slot.worker.postMessage({ type: 'mesh', id, cell: cell.node, rims: all } satisfies PlanRequest, [all.buffer]);
    }

    /** Whether a cell's block is to come: the cell can still get rows, and its block is not asked yet, on its way, or waits for a nearer one. */
    private coming(name: string): boolean {
        const cell = this.cells.get(name);
        return this.open.has(name) && (!cell || cell.state === 'asked' || (cell.state === 'over' && !!cell.words));
    }

    /** Whether a page of a near node waits for its cell's block. Nothing waits for a cell that gets no rows, or once the records failed. */
    waits(node: Node): boolean {
        return node.level >= CELL_LEVEL && !this.error && this.coming(key(cellOf(node)));
    }

    /** Whether the pages of a cell wait for its mesh: the mesh wants the cell, it is not settled, and no kept input of it was lost. */
    meshComing(node: Node): boolean {
        const name = key(node);
        return !this.error && this.meshWanted.has(name) && !this.cells.get(name)?.lost && !this.settled(node);
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

    /** Whether a cell is settled for the mesh: meshed, or with no mesh to come, as a cell that gets no rows. */
    settled(node: Node): boolean {
        const name = key(node), cell = this.cells.get(name);
        return cell?.state === 'built' ? cell.mesh === 'done' : !this.coming(name);
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

    /** A new device holds no block: every cell with rows asks again. */
    lost(): void {
        for (const cell of this.cells.values()) this.evict(cell);
        this.changed = [];
        this.uploads = [];
        this.tableDirty = true;
    }

    /** True while a block is to come, or a mesh cell is not settled. False once the records failed, so a wait for an idle view ends. */
    get loading(): boolean {
        if (this.error) return false;
        return [...this.open].some(name => this.coming(name)) || [...this.meshWanted].some(name => !this.settled(nodeOf(name)));
    }

    get stats(): RecordsStats {
        const built = [...this.cells.values()].flatMap(c => (c.state === 'built' ? [c] : []));
        const median = (values: number[]) => [...values].sort((a, b) => a - b)[Math.floor(values.length / 2)] ?? 0;
        return {
            cells: built.length, bytes: built.reduce((sum, c) => sum + c.rows * MODEL_ROW * 4, 0), radius: Math.max(0, ...built.map(c => c.distance)), wanted: this.wantedM,
            building: this.asked.length + this.pool.reduce((sum, slot) => sum + (slot?.jobs.size ?? 0), 0), build: median(this.times), mesh: median(this.meshTimes), hits: this.hits, cacheBytes: this.opened?.bytes ?? 0,
            restarts: this.restarts, heap: this.heap, failed: [...this.cells.values()].filter(c => c.state === 'failed' || c.farFailed).length,
            builds: this.builds, refused: this.refused, large: this.large,
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
