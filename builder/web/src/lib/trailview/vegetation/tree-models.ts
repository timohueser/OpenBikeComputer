import { dayOfYear } from '../ground/rules';
import type { Instances, ModelSpec } from '../instances/instances';
import { SEASONS } from '../rules/regions';
import type { BuiltLeafy, BuiltModel, ModelAnswer, ModelJob } from './model-worker';
import { phasesOn } from './phenology';

/** Trees stand within this many metres of the eye: the stamps of main roads reach 4 km. */
export const TREE_REACH_M = 4500;
/**
 * The mesh tiers of a tree: three levels of detail, and where each ends for a tree TREE_SIZE_M tall; a
 * tree of another height switches where it draws as large. Impostors follow to the reach.
 */
const TIERS_M = [60, 300, 700], TREE_SIZE_M = 25;
/** Workers that build the models side by side, a species or a crown per job: the trees show as each job lands. */
const WORKERS = 3;

/** An instance model of a species, and the models of its phases that draw a crown of their own. */
interface Entry { index: number; own: (number | undefined)[] }

/**
 * The tree models (model-worker.ts), one build per species for the forest, the shrubs and the
 * orchards. A tree draws the summer crown, which its phase on the date (phenology.ts) colours, or a
 * crown of the phase's own, such as a bare one, once the workers have built it for a phase that the
 * date has; until then the summer crown in the phase's colours. A free worker takes the next build:
 * summer crowns before crowns of their own, each with the models about the eye first. Built models
 * stay, so a change of date builds only what it has not seen. A species that a pack keeps green all
 * year stays in summer. A model with a tie to the ground's hue (draw.wgsl), such as a tree that stands
 * as a shrub, is an instance model of its own on the same meshes.
 */
export class TreeModels {
    /** Instance models by `model|evergreen|tie`. */
    private readonly entries = new Map<string, Entry>();
    private readonly workers = Array.from({ length: WORKERS }, () => new Worker(new URL('./model-worker.ts', import.meta.url), { type: 'module' }));
    private readonly free = [...this.workers];
    /** The jobs that run, by id. */
    private readonly jobs = new Map<number, Omit<ModelJob, 'id'>>();
    /** The built summer crowns by `model|evergreen`, the built crowns of a phase by `model|phase`, what is asked for (`leafy:` or `crown:` and the key), and the builds that wait for a worker. */
    private readonly leafy = new Map<string, BuiltLeafy>();
    private readonly crowns = new Map<string, BuiltModel>();
    private readonly asked = new Set<string>();
    private waiting: Omit<ModelJob, 'id'>[] = [];
    /** The phases that the date has. */
    private phases = new Set<number>();
    private day?: number;
    private next = 0;
    /** Whether the instances lack models that arrived, or the date's phases: the next update installs them, once a frame. */
    private stale = false;
    /** Whether models or the date changed in the last update. */
    newContent = false;

    /** `near` gives the models about the eye, each with its share of the trees there. An abort of `stop` ends the workers. */
    constructor(private readonly instances: Instances, private readonly wake: () => void, private readonly near: () => Map<string, number>, stop: AbortSignal) {
        stop.addEventListener('abort', () => this.workers.forEach(worker => worker.terminate()));
        for (const worker of this.workers) worker.onmessage = ({ data }: MessageEvent<ModelAnswer>) => this.land(worker, data);
    }

    get idle(): boolean {
        return this.jobs.size === 0 && this.waiting.length === 0 && !this.stale;
    }

    /** The instance model of a tree species with a tie to the ground's hue; undefined where the instances hold no more models. */
    index(model: string, evergreen = false, tie = 0): number | undefined {
        const key = `${model}|${evergreen ? 1 : 0}|${tie}`;
        if (!this.entries.has(key)) {
            const index = this.instances.add({ tiers: [] });
            if (index === undefined) return undefined;
            this.entries.set(key, { index, own: [] });
        }
        return this.entries.get(key)!.index;
    }

    /** Takes the date: the trees change phase at once, and the workers build what its phases lack. */
    update(date: string): void {
        this.newContent = false;
        const day = dayOfYear(date);
        if (day !== this.day && this.entries.size) {
            this.day = this.instances.day = day;
            this.newContent = true;
            const phases = phasesOn(day);
            if ([...phases].sort().join() !== [...this.phases].sort().join()) {
                this.phases = phases;
                this.stale = true;
                this.ask();
            }
        }
        if (!this.stale) return;
        this.stale = false;
        this.newContent = true;
        this.install();
    }

    /** Takes a job's models; a job that failed is asked for again when the next job lands or the date's phases change. */
    private land(worker: Worker, { id, leafy, crowns, error }: ModelAnswer): void {
        const job = this.jobs.get(id)!;
        this.jobs.delete(id);
        this.free.push(worker);
        if (error || !leafy || !crowns) {
            job.leafy.forEach(({ model, evergreen }) => this.asked.delete(`leafy:${model}|${evergreen ? 1 : 0}`));
            job.crowns.forEach(({ model, phase }) => this.asked.delete(`crown:${model}|${phase}`));
            this.post();
            return void console.warn(`Tree models: ${error}`);
        }
        job.leafy.forEach(({ model, evergreen }, i) => this.leafy.set(`${model}|${evergreen ? 1 : 0}`, leafy[i]));
        job.crowns.forEach(({ model, phase }, i) => this.crowns.set(`${model}|${phase}`, crowns[i]));
        this.stale = true;
        this.ask();
        this.wake();
    }

    /** Queues the summer crowns not yet asked for, and the crowns of their own that the date's phases need. */
    private ask(): void {
        const leafy = this.builds().filter(build => !this.asked.has(`leafy:${build}`)).map(build => {
            this.asked.add(`leafy:${build}`);
            const [model, evergreen] = build.split('|');
            return { model, evergreen: evergreen === '1' };
        });
        const crowns = [...this.leafy].flatMap(([build, { own }]) => [...this.phases].filter(phase => own[phase]).map(phase => `${build.split('|')[0]}|${phase}`))
            .filter(crown => !this.asked.has(`crown:${crown}`)).map(crown => {
                this.asked.add(`crown:${crown}`);
                const [model, phase] = crown.split('|');
                return { model, phase: Number(phase) };
            });
        this.waiting.push(...leafy.map(build => ({ leafy: [build], crowns: [] })), ...crowns.map(crown => ({ leafy: [], crowns: [crown] })));
        this.post();
    }

    /** Gives the free workers the waiting builds: summer crowns first, each with the models that have the most trees about the eye first. */
    private post(): void {
        if (!this.free.length || !this.waiting.length) return;
        // Crowns of phases that the date lost: `ask()` queues them again if the date comes back.
        for (const { model, phase } of this.waiting.flatMap(job => job.crowns)) if (!this.phases.has(phase)) this.asked.delete(`crown:${model}|${phase}`);
        this.waiting = this.waiting.filter(job => job.crowns.every(({ phase }) => this.phases.has(phase)));
        const near = this.near(), share = (job: Omit<ModelJob, 'id'>) => near.get((job.leafy[0] ?? job.crowns[0]).model) ?? 0;
        this.waiting.sort((a, b) => b.leafy.length - a.leafy.length || share(b) - share(a));
        for (const job of this.waiting.splice(0, this.free.length)) {
            const id = this.next++;
            this.jobs.set(id, job);
            this.free.pop()!.postMessage({ id, ...job } satisfies ModelJob);
        }
    }

    /**
     * Gives each species that has its summer crown its models: a phase that needs a crown of its own
     * draws it once built, else the summer crown in the phase's colours. A crown of its own that differs
     * only in colour draws the summer crown's impostor. Crowns of phases that the date lacks leave the pools.
     */
    private install(): void {
        // Crowns of their own take their instance models in entry and phase order once every species
        // has its summer crown, so the indexes never hang on which worker answers first.
        const settled = this.builds().every(build => this.leafy.has(build));
        for (const [key, entry] of this.entries) {
            const [model, evergreen, tie] = key.split('|'), leafy = this.leafy.get(`${model}|${evergreen}`);
            if (!leafy) continue;
            const spec = (crown: BuiltModel): ModelSpec => ({
                tiers: crown.tiers.map((mesh, t) => ({ mesh, until: TIERS_M[t] })), impostor: { from: 2, until: TREE_REACH_M }, crown: crown.crown, tie: Number(tie), sized: TREE_SIZE_M, luma: leafy.luma,
            });
            const own = SEASONS.map((_, phase) => {
                if (!this.phases.has(phase) || !leafy.own[phase]) return undefined;
                if (settled) entry.own[phase] ??= this.instances.add({ tiers: [] });
                return this.crowns.get(`${model}|${phase}`);
            });
            entry.own.forEach((index, phase) => index !== undefined && this.instances.replace(index, own[phase] ? spec(own[phase]) : { tiers: [] }));
            this.instances.replace(entry.index, {
                ...spec(leafy), phases: evergreen === '1' ? undefined : SEASONS.map((_, phase) => {
                    const tone = leafy.tones[phase], model = own[phase] && entry.own[phase];
                    return model === undefined ? { model: entry.index, tone } : { model, impostor: tone ? entry.index : model, tone };
                }),
            });
        }
    }

    /** The builds, by `model|evergreen`. */
    private builds(): string[] {
        return [...new Set([...this.entries.keys()].map(key => key.split('|').slice(0, 2).join('|')))];
    }
}
