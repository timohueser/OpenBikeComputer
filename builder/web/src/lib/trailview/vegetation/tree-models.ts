import { dayOfYear } from '../ground/rules';
import type { Instances, ModelSpec } from '../instances/instances';
import { SEASONS } from '../rules/regions';
import type { BuiltLeafy, BuiltModel, ModelJob } from './model-worker';
import { phasesOn } from './phenology';

/** Trees stand within this many metres of the eye: the stamps of main roads reach 4 km. */
export const TREE_REACH_M = 4500;
/**
 * The mesh tiers of a tree: three levels of detail, and where each ends for a tree TREE_SIZE_M tall; a
 * tree of another height switches where it draws as large. Impostors follow to the reach.
 */
const TIERS_M = [60, 300, 700], TREE_SIZE_M = 25;
/** Workers that build the models side by side, and species or crowns per job: the trees show as each job lands. */
const WORKERS = 3, PER_JOB = 4;

/** An instance model of a species, and the models of its phases that draw a crown of their own. */
interface Entry { index: number; own: (number | undefined)[] }

/**
 * The tree models (model-worker.ts), one build per species for the forest, the shrubs and the
 * orchards. A tree draws the summer crown, which its phase on the date (phenology.ts) colours, or a
 * crown of the phase's own, such as a bare one, once the workers have built it for a phase that the
 * date has; until then the summer crown in the phase's colours. Built models stay, so a change of date
 * builds only what it has not seen. A species that a pack keeps green all year stays in summer. A
 * model with a tie to the ground's hue (draw.wgsl), such as a tree that stands as a shrub, is an
 * instance model of its own on the same meshes.
 */
export class TreeModels {
    /** Instance models by `model|evergreen|tie`. */
    private readonly entries = new Map<string, Entry>();
    private readonly workers = Array.from({ length: WORKERS }, () => new Worker(new URL('./model-worker.ts', import.meta.url), { type: 'module' }));
    /** The jobs that run, by id. */
    private readonly jobs = new Map<number, Omit<ModelJob, 'id'>>();
    /** The built summer crowns by `model|evergreen`, the built crowns of a phase by `model|phase`, and what the jobs ask for: `leafy:` or `crown:` and the key. */
    private readonly leafy = new Map<string, BuiltLeafy>();
    private readonly crowns = new Map<string, BuiltModel>();
    private readonly asked = new Set<string>();
    /** The phases that the date has. */
    private phases = new Set<number>();
    private day?: number;
    private next = 0;
    private rebuilt = false;
    /** Whether models arrived since the last update. */
    newContent = false;

    constructor(private readonly instances: Instances, private readonly wake: () => void) {
        for (const worker of this.workers) {
            worker.onmessage = ({ data }: MessageEvent<{ id: number; leafy?: BuiltLeafy[]; crowns?: BuiltModel[]; error?: string }>) => {
                const job = this.jobs.get(data.id)!;
                this.jobs.delete(data.id);
                if (data.error || !data.leafy || !data.crowns) return void console.warn(`Tree models: ${data.error}`);
                job.leafy.forEach(({ model, evergreen }, i) => this.leafy.set(`${model}|${evergreen ? 1 : 0}`, data.leafy![i]));
                job.crowns.forEach(({ model, phase }, i) => this.crowns.set(`${model}|${phase}`, data.crowns![i]));
                this.install();
                this.ask();
            };
        }
    }

    get idle(): boolean {
        return this.jobs.size === 0;
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
        this.newContent = this.rebuilt;
        this.rebuilt = false;
        const day = dayOfYear(date);
        if (day === this.day || !this.entries.size) return;
        this.day = this.instances.day = day;
        this.newContent = true;
        const phases = phasesOn(day);
        if ([...phases].sort().join() === [...this.phases].sort().join()) return;
        this.phases = phases;
        this.install();
        this.ask();
    }

    /** Posts jobs for the summer crowns not yet asked for, and for the crowns of their own that the date's phases need. */
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
        for (let k = 0; k < leafy.length; k += PER_JOB) this.post({ leafy: leafy.slice(k, k + PER_JOB), crowns: [] });
        for (let k = 0; k < crowns.length; k += PER_JOB) this.post({ leafy: [], crowns: crowns.slice(k, k + PER_JOB) });
    }

    private post(job: Omit<ModelJob, 'id'>): void {
        const id = this.next++;
        this.jobs.set(id, job);
        this.workers[id % WORKERS].postMessage({ id, ...job } satisfies ModelJob);
    }

    /**
     * Gives each species that has its summer crown its models: a phase that needs a crown of its own
     * draws it once built, else the summer crown in the phase's colours. A crown of its own that differs
     * only in colour draws the summer crown's impostor. Crowns of phases that the date lacks leave the pools.
     */
    private install(): void {
        for (const [key, entry] of this.entries) {
            const [model, evergreen, tie] = key.split('|'), leafy = this.leafy.get(`${model}|${evergreen}`);
            if (!leafy) continue;
            const spec = (crown: BuiltModel): ModelSpec => ({
                tiers: crown.tiers.map((mesh, t) => ({ mesh, until: TIERS_M[t] })), impostor: { from: 2, until: TREE_REACH_M }, crown: crown.crown, tie: Number(tie), sized: TREE_SIZE_M,
            });
            const own = SEASONS.map((_, phase) => (this.phases.has(phase) && leafy.own[phase] ? this.crowns.get(`${model}|${phase}`) : undefined));
            own.forEach((crown, phase) => {
                if (crown) entry.own[phase] ??= this.instances.add({ tiers: [] });
                if (entry.own[phase] !== undefined) this.instances.replace(entry.own[phase]!, crown ? spec(crown) : { tiers: [] });
            });
            this.instances.replace(entry.index, {
                ...spec(leafy), phases: evergreen === '1' ? undefined : SEASONS.map((_, phase) => {
                    const tone = leafy.tones[phase], model = own[phase] && entry.own[phase];
                    return model === undefined ? { model: entry.index, tone } : { model, impostor: tone ? entry.index : model, tone };
                }),
            });
        }
        this.rebuilt = true;
        this.wake();
    }

    /** The builds, by `model|evergreen`. */
    private builds(): string[] {
        return [...new Set([...this.entries.keys()].map(key => key.split('|').slice(0, 2).join('|')))];
    }
}
