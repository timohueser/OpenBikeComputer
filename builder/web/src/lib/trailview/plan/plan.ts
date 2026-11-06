// The ground plan about the eye, with `ground=plan`: picks the cells, collects their inputs, and builds
// them in one plan worker. The main thread's instance of the bridge only names inputs; no build runs
// here, because one synchronous build would block the frames.
import { stopped } from '../stream/terrain';
import { distances, key, size, type Node } from '../terrain/lod';
import type { FeatureTiles } from '../terrain/stamps';
import { failedMesh, type DebugMesh, type Vocabulary } from './debug';
import { PlanDraw } from './draw';
import { cellInputs, type StoredTiles } from './inputs';
import init, { feature_tiles, vocabulary, wanted } from './pkg/obc_ground_plan_bridge.js';
import type { Built, Failed, PlanRequest } from './plan-worker';

const WASM = new URL('./pkg/obc_ground_plan_bridge_bg.wasm', import.meta.url);
/** The level of the nodes that are cells: zoom 16. */
export const CELL_LEVEL = 16;
/** The mesh range of the ground plan. */
export const BUILD_M = 600;
/** A cell builds this far before it comes into range: as AHEAD_M of the roads, about 20 s of a car. */
export const AHEAD_M = 300;
/** "Ahead" is within 60° of the view direction. */
const AHEAD_COS = 0.5;
/** A cell drops past this: 100 m past BUILD_M + AHEAD_M, so a cell at the edge does not build again when the eye moves to and fro. */
const KEEP_M = 1000;
/** The CPU bytes of the records: as MESH_BYTES of the roads. */
const KEPT_BYTES = 32 * 2 ** 20;
/** The epic's worker heap. WASM memory never shrinks, so a worker past it ends after its job and a new one starts. */
export const HEAP_LIMIT = 55 * 2 ** 20;
/** Jobs that the worker holds: one runs and one waits, so it does not idle while the main thread collects inputs. */
const JOBS = 2;
/** The builds that the build times cover. */
const TIMES = 32;
/** What a record costs besides its debug mesh: its stats and times. */
const RECORD_BYTES = 2048;

/** The eye: world units, metres per unit, and its height above the ground. */
export interface PlanEye { x: number; y: number; unit: number; above: number }

type Entry = { node: Node; bytes: number } & (
    | { state: 'built'; stats: string; times: number[]; digest?: string; request: string; debug?: DebugMesh }
    | { state: 'empty' }
    | { state: 'failed'; error: string });

export interface PlanStats {
    cells: number;
    building: number;
    failed: number;
    restarts: number;
    /** Milliseconds from a frame with a cell in range without its record to the next frame with them all. */
    ready: number;
    /**
     * Over the last TIMES builds, in ms: the median and the 95th percentile of the WASM stages' total,
     * the median of each stage, and the median of `prepare`, the worker's gunzip and WebP decode before them.
     */
    build: { median: number; p95: number; stages: Record<string, number>; prepare: number };
    /** The largest heap of a worker. */
    heap: number;
    debugVertices: number;
    /** Faces of the drawn cells that `earcut` left unfilled. */
    unfilled: number;
    /** Per built cell with the debug draw, `x-y`: the digests of its request and its plan. */
    digests: Record<string, { request: string; plan: string }>;
    error?: string;
}

const quantile = (values: number[], q: number) => {
    const sorted = [...values].sort((a, b) => a - b);
    return sorted.length ? sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))] : 0;
};

/**
 * The cells to build, by priority: every cell whose square meets `bounds` (world units: west, north,
 * east, south) nearer than BUILD_M, by distance; then those up to BUILD_M + AHEAD_M whose middle lies
 * ahead, by distance; none for an eye BUILD_M or more above the ground. `ahead` is the horizontal view direction in world units (x east, y south), or
 * undefined when the view looks nearly straight down or up.
 */
export function wantedCells(eye: PlanEye, ahead: [number, number] | undefined, bounds: [number, number, number, number]): Node[] {
    // An eye above the mesh range sees no plan.
    if (eye.above >= BUILD_M) return [];
    const s = size(CELL_LEVEL), reach = (BUILD_M + AHEAD_M) / eye.unit, last = 2 ** CELL_LEVEL - 1;
    const anchor = { whole: [eye.x, eye.y] as [number, number], part: [0, 0] as [number, number], unit: eye.unit };
    const near: [Node, number][] = [], far: [Node, number][] = [];
    const span = (v: number) => [Math.max(0, Math.floor((v - reach) / s)), Math.min(last, Math.floor((v + reach) / s))];
    const [x0, x1] = span(eye.x), [y0, y1] = span(eye.y);
    for (let y = y0; y <= y1; y++) for (let x = x0; x <= x1; x++) {
        if (x * s >= bounds[2] || (x + 1) * s <= bounds[0] || y * s >= bounds[3] || (y + 1) * s <= bounds[1]) continue;
        const node = { level: CELL_LEVEL, x, y }, d = distances({ anchor, above: eye.above }, node)[0];
        if (d < BUILD_M) near.push([node, d]);
        else if (d < BUILD_M + AHEAD_M && ahead) {
            const mx = (x + 0.5) * s - eye.x, my = (y + 0.5) * s - eye.y, length = Math.hypot(mx, my);
            if (length > 0 && (mx * ahead[0] + my * ahead[1]) / length >= AHEAD_COS) far.push([node, d]);
        }
    }
    const order = (list: [Node, number][]) => list.sort((a, b) => a[1] - b[1]).map(([node]) => node);
    return [...order(near), ...order(far)];
}

export class Plan {
    /** Whether the debug draw is on: builds then carry debug meshes and digests. */
    debug = false;
    readonly draw = new PlanDraw();
    private records = new Map<string, Entry>();
    /** The bytes of each cell's last record, kept past an eviction so that the cell is not asked again while it would not fit. */
    private sizes = new Map<string, number>();
    /** The cells that collect inputs or wait for the worker, with the generation of the worker. */
    private pending = new Map<string, number>();
    private jobs = new Map<number, string>();
    private worker?: Worker;
    private generation = 0;
    private ready = false;
    private module?: WebAssembly.Module;
    private words?: Vocabulary;
    private error?: string;
    private next = 0;
    private restarts = 0;
    private heap = 0;
    private times: { stages: number[]; prepare: number }[] = [];
    private dirty = true;
    private missing = true;
    private readyFrom?: number;
    private readyMs = 0;
    private eye?: PlanEye;

    /** `stored` gives the stored terrain tiles; an abort of `stop` ends the worker. */
    constructor(private readonly features: FeatureTiles | undefined, private readonly stored: StoredTiles, private readonly bounds: [number, number, number, number], private readonly changed: () => void, private readonly stop: AbortSignal) {
        stop.addEventListener('abort', () => this.worker?.terminate());
        if (!features) {
            this.error = 'the release has no feature tiles';
            return;
        }
        void this.load();
    }

    private async load(): Promise<void> {
        try {
            // One download and one compile: the worker takes the module.
            const module = await WebAssembly.compile(await (await fetch(WASM, { signal: this.stop })).arrayBuffer());
            await init({ module_or_path: module });
            this.words = JSON.parse(vocabulary());
            this.module = module;
            this.spawn();
        } catch (error) {
            if (stopped(error)) return;
            this.error = `the plan bridge did not start: ${error instanceof Error ? error.message : error}`;
            this.changed();
        }
    }

    private spawn(): void {
        if (this.stop.aborted) return;
        const worker = this.worker = new Worker(new URL('./plan-worker.ts', import.meta.url), { type: 'module' });
        this.ready = false;
        this.generation++;
        worker.onmessage = ({ data }) => this.receive(worker, data);
        worker.onerror = event => {
            this.error = `the plan worker failed: ${event.message}`;
            this.changed();
        };
        worker.postMessage({ type: 'start', module: this.module! } satisfies PlanRequest);
    }

    /** Ends the worker and starts another; the jobs that it held go back to not asked. */
    private restart(): void {
        this.worker?.terminate();
        this.restarts++;
        this.pending.clear();
        this.jobs.clear();
        this.spawn();
    }

    private receive(worker: Worker, data: { type: 'ready'; error?: string } | Built | Failed): void {
        if (worker !== this.worker) return;
        if ('type' in data) {
            if (data.error) this.error = `the plan worker did not start: ${data.error}`;
            else this.ready = true;
            return this.changed();
        }
        const name = this.jobs.get(data.id);
        this.jobs.delete(data.id);
        if (name === undefined) return;
        this.pending.delete(name);
        const node = nodeOf(name);
        if (data.ok) {
            this.times = [...this.times.slice(1 - TIMES), { stages: data.times, prepare: data.prepare }];
            this.heap = Math.max(this.heap, data.heap);
        }
        // A trapped instance or a full heap ends the worker, whatever happens to the result.
        if (data.ok ? data.heap > HEAP_LIMIT : data.trapped) this.restart();
        if (!this.eye || distance(this.eye, node) > KEEP_M) return this.changed();
        if (data.ok) {
            const request = (JSON.parse(data.stats) as { inputs: { request: string } }).inputs.request;
            this.store(name, { node, state: 'built', stats: data.stats, times: data.times, digest: data.digest, request, debug: data.debug, bytes: RECORD_BYTES + (data.debug ? data.debug.lines.byteLength + data.debug.fills.byteLength : 0) });
        } else this.store(name, { node, state: 'failed', error: data.error, bytes: RECORD_BYTES });
        this.dirty = true;
        this.changed();
    }

    private store(name: string, record: Entry): void {
        this.records.set(name, record);
        this.sizes.set(name, record.bytes);
    }

    /** A record that serves: with the debug draw on, a build without its debug mesh counts as missing. */
    private serves(record: Entry | undefined): boolean {
        return !!record && !(this.debug && record.state === 'built' && !record.debug);
    }

    /**
     * Picks the cells for the eye, drops the far records and asks the worker for the next cells while
     * `build` lets it. Returns whether the drawn debug meshes changed.
     */
    update(eye: PlanEye, ahead: [number, number] | undefined, build: boolean): boolean {
        this.eye = eye;
        if (this.error && !this.module) return false;
        const cells = wantedCells(eye, ahead, this.bounds);
        for (const [name, record] of this.records) {
            if (distance(eye, record.node) > KEEP_M) {
                this.records.delete(name);
                this.dirty = true;
            }
        }
        for (const name of this.sizes.keys()) if (distance(eye, nodeOf(name)) > KEEP_M) this.sizes.delete(name);
        // The records of the wanted cells first, by priority, then the others by distance. The kept
        // records are a prefix of that order: from the first that does not fit, all go.
        const wantedNames = cells.map(key), wantedSet = new Set(wantedNames);
        const others = [...this.records.keys()].filter(name => !wantedSet.has(name)).sort((a, b) => distance(eye, nodeOf(a)) - distance(eye, nodeOf(b)));
        let bytes = 0, full = false;
        for (const name of [...wantedNames, ...others]) {
            const record = this.records.get(name);
            if (!record) continue;
            if (full || bytes + record.bytes > KEPT_BYTES) {
                full = true;
                this.records.delete(name);
                this.dirty = true;
            } else bytes += record.bytes;
        }
        full = false;
        let missing = false;
        for (const node of cells) {
            const name = key(node), record = this.records.get(name);
            if (this.serves(record)) continue;
            // A cell asks only while its last size fits: else it would build, go and build again every frame.
            if (bytes + (this.sizes.get(name) ?? RECORD_BYTES) > KEPT_BYTES) {
                full = true;
                break;
            }
            if (distance(eye, node) < BUILD_M) missing = true;
            if (!build || !this.ready || this.pending.has(name) || this.pending.size >= JOBS) continue;
            void this.ask(node);
        }
        this.missing = missing && !full;
        const now = performance.now();
        if (this.missing) this.readyFrom ??= now;
        else if (this.readyFrom !== undefined) {
            this.readyMs = now - this.readyFrom;
            this.readyFrom = undefined;
        }
        if (!this.dirty) return false;
        this.dirty = false;
        const meshes = [...this.records.values()].flatMap(record => (record.state === 'built' && record.debug ? [record.debug] : record.state === 'failed' ? [failedMesh(record.node)] : []));
        this.draw.set(meshes);
        return this.debug;
    }

    private async ask(node: Node): Promise<void> {
        const name = key(node), generation = this.generation;
        this.pending.set(name, generation);
        try {
            const inputs = await cellInputs(node, { feature_tiles, wanted, words: this.words!.words.wanted }, this.features!, this.stored);
            if (this.pending.get(name) !== generation) return;
            if (inputs === 'empty') {
                this.pending.delete(name);
                this.store(name, { node, state: 'empty', bytes: RECORD_BYTES });
            } else {
                const id = this.next++;
                this.jobs.set(id, name);
                const message: PlanRequest = { type: 'build', id, cell: node, ...inputs, debug: this.debug };
                this.worker!.postMessage(message, inputs.tiles.map(({ body }) => body));
                return;
            }
        } catch (error) {
            if (stopped(error) || this.pending.get(name) !== generation) return;
            this.pending.delete(name);
            this.store(name, { node, state: 'failed', error: error instanceof Error ? error.message : String(error), bytes: RECORD_BYTES });
        }
        this.dirty = true;
        this.changed();
    }

    /** True while a cell in range has no record. False when the bridge failed to start, so a wait for an idle view ends. */
    get loading(): boolean {
        return !this.error && this.missing;
    }

    /** The bridge's names and colours, once it started. */
    get vocabulary(): Vocabulary | undefined {
        return this.words;
    }

    get stats(): PlanStats {
        const records = [...this.records.values()], built = records.flatMap(r => (r.state === 'built' ? [r] : []));
        const stages = Object.fromEntries((this.words?.stages ?? []).map((stage, i) => [stage, quantile(this.times.map(t => t.stages[i]), 0.5)]));
        const totals = this.times.map(t => t.stages.reduce((sum, ms) => sum + ms, 0)), prepare = quantile(this.times.map(t => t.prepare), 0.5);
        return {
            cells: built.length, building: this.pending.size, failed: records.filter(r => r.state === 'failed').length, restarts: this.restarts, ready: this.readyMs,
            build: { median: quantile(totals, 0.5), p95: quantile(totals, 0.95), stages, prepare }, heap: this.heap, debugVertices: this.draw.vertices,
            unfilled: built.reduce((sum, r) => sum + (r.debug?.unfilled ?? 0), 0),
            digests: Object.fromEntries(built.flatMap(r => (r.digest ? [[`${r.node.x}-${r.node.y}`, { request: r.request, plan: r.digest }]] : []))),
            ...(this.error ? { error: this.error } : {}),
        };
    }
}

const nodeOf = (name: string): Node => {
    const [level, x, y] = name.split('/').map(Number);
    return { level, x, y };
};

/** The metres from the eye to a cell's square, with the eye's height above the ground. */
const distance = (eye: PlanEye, node: Node) => distances({ anchor: { whole: [eye.x, eye.y], part: [0, 0], unit: eye.unit }, above: eye.above }, node)[0];
