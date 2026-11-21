import { dayOfYear } from '../ground/rules';
import type { Instances, ModelSpec } from '../instances/instances';
import type { ModelFiles } from '../instances/model-files';
import type { Packed } from '../models/model-file';
import { SEASONS } from '../rules/regions';
import { phasesOn } from './phenology';

/** Trees stand within this many metres of the eye: the stamps of main roads reach 4 km. */
export const TREE_REACH_M = 4500;
/**
 * The mesh tiers of a tree: three levels of detail, and where each ends for a tree TREE_SIZE_M tall; a
 * tree of another height switches where it draws as large. Impostors follow to the reach.
 */
const TIERS_M = [60, 300, 700], TREE_SIZE_M = 25;

/** An instance model of a species, the models of its phases that draw a crown of their own, and the regions whose trees take it. */
interface Entry { index: number; own: (number | undefined)[]; regions: Set<number> }
/** The regions whose trees and plants the view can show (`reach`), and those of the near field (`near`). */
export interface Reach { reach: Set<number>; near: Set<number> }
/** Whether two sets of regions share one. */
export function meets(a: Set<number>, b: Set<number>): boolean {
    for (const region of a) if (b.has(region)) return true;
    return false;
}
/** A file to load: a species' summer crown, or a crown of a species in a phase. */
interface Job { model: string; phase?: number }

/**
 * The tree models (models/model-file.ts), one file per species for the forest, the shrubs and the plantings, of the
 * species of the regions in reach. A tree draws the summer crown, which its phase on the date (phenology.ts) colours, or a
 * crown of the phase's own, such as a bare one, once its file has arrived for a phase that the date has; until then the
 * summer crown in the phase's colours. At most `loads` files load at once: summer crowns before crowns of their own, each
 * with the models of the near regions and then about the eye first. Loaded models stay, so a change of date loads only
 * what it has not seen. A file that failed counts as done, and is asked for again when the next file lands or the
 * date's phases change; ModelFiles answers at once for a file that may not pass. A species that a pack keeps green all year stays in summer. A model with a tie to the ground's hue
 * (draw.wgsl), such as a tree that stands as a shrub, is an instance model of its own on the same meshes.
 */
export class TreeModels {
    /** Instance models by `model|evergreen|tie`. */
    private readonly entries = new Map<string, Entry>();
    /** The summer crowns by model, and the crowns of a phase by `model|phase`, that arrived. */
    private readonly leafy = new Map<string, Packed>();
    private readonly crowns = new Map<string, Packed>();
    /** The jobs asked for (`model` or `model|phase`), those that failed since the last `ask`, and those that wait for a load. */
    private readonly asked = new Set<string>();
    private readonly failed = new Set<string>();
    private waiting: Job[] = [];
    private loading = 0;
    /** The phases that the date has. */
    private phases = new Set<number>();
    private day?: number;
    /** Whether the instances lack models that arrived, or the date's phases: the next update installs them, once a frame. */
    private stale = false;
    /** Whether models or the date changed in the last update. */
    newContent = false;
    /** The regions in reach at the last update, and the builds of a set of regions (`builds`). */
    private wanted: Reach = { reach: new Set(), near: new Set() };
    private buildsOf = new WeakMap<Set<number>, string[]>();

    /** `near` gives the models about the eye, each with its share of the trees there. */
    constructor(private readonly instances: Instances, private readonly near: () => Map<string, number>, private readonly files: ModelFiles, private readonly loads: number) {}

    get idle(): boolean {
        return this.loading === 0 && this.waiting.length === 0 && !this.stale && this.builds().every(build => this.asked.has(build.split('|')[0]) || this.failed.has(build.split('|')[0]));
    }

    /**
     * Whether the trees of the near regions draw their final models: each has its summer crown, and the crown of its own of
     * each of the date's phases that needs one is installed. A file that failed counts as done.
     */
    get nearIdle(): boolean {
        if (this.stale) return false;
        for (const [key, entry] of this.entries) {
            if (!meets(entry.regions, this.wanted.near)) continue;
            const [model, evergreen] = key.split('|');
            if (this.failed.has(model)) continue;
            if (!this.leafy.has(model)) return false;
            if (evergreen === '1') continue;
            for (const phase of this.phases) {
                const crown = `${model}|${phase}`;
                if (this.ownIn(model, phase) && !this.failed.has(crown) && (!this.crowns.has(crown) || entry.own[phase] === undefined)) return false;
            }
        }
        return true;
    }

    /**
     * The instance model of a tree species with a tie to the ground's hue, for the trees of region `region`; undefined where
     * the instances hold no more models.
     */
    index(region: number, model: string, evergreen = false, tie = 0): number | undefined {
        const key = `${model}|${evergreen ? 1 : 0}|${tie}`;
        if (!this.entries.has(key)) {
            const index = this.instances.add({ tiers: [] });
            if (index === undefined) return undefined;
            this.entries.set(key, { index, own: [], regions: new Set() });
        }
        const entry = this.entries.get(key)!;
        entry.regions.add(region);
        this.buildsOf = new WeakMap();
        return entry.index;
    }

    /** Takes the date and the regions in reach: the trees change phase at once, and the files that they lack load. */
    update(date: string, reach: Reach): void {
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
        if (reach !== this.wanted) {
            this.wanted = reach;
            if (this.builds().some(build => !this.asked.has(build.split('|')[0]) && !this.failed.has(build.split('|')[0]))) this.ask();
        }
        if (!this.stale) return;
        this.stale = false;
        this.newContent = true;
        this.install();
    }

    /** Whether a species draws a crown of its own in a phase. */
    private ownIn(model: string, phase: number): boolean {
        return !!this.files.models.trees[model]?.crowns[phase];
    }

    /** Queues the summer crowns not yet asked for, and the crowns of their own that the date's phases need. */
    private ask(): void {
        this.failed.clear();
        const leafy = [...new Set(this.builds().map(build => build.split('|')[0]))].filter(model => !this.asked.has(model)).map((model): Job => ({ model }));
        // A species that every pack keeps green all year needs no crown of its own.
        const seasonal = new Set(this.builds().filter(build => build.endsWith('|0')).map(build => build.split('|')[0]));
        const crowns = [...this.leafy.keys()].filter(model => seasonal.has(model)).flatMap(model => [...this.phases].filter(phase => this.ownIn(model, phase)).map((phase): Job => ({ model, phase })))
            .filter(job => !this.asked.has(`${job.model}|${job.phase}`));
        for (const job of [...leafy, ...crowns]) {
            this.asked.add(keyOf(job));
            this.waiting.push(job);
        }
        this.post();
    }

    /**
     * Starts the waiting loads while fewer than `loads` run: summer crowns first, each with the models of the near regions
     * first, then those that have the most trees about the eye.
     */
    private post(): void {
        // Crowns of phases that the date lost: `ask()` queues them again if the date comes back.
        for (const job of this.waiting) if (job.phase !== undefined && !this.phases.has(job.phase)) this.asked.delete(keyOf(job));
        this.waiting = this.waiting.filter(job => job.phase === undefined || this.phases.has(job.phase));
        if (this.loading >= this.loads || !this.waiting.length) return;
        const near = this.near(), close = new Set(this.builds(this.wanted.near).map(build => build.split('|')[0]));
        const share = (job: Job) => near.get(job.model) ?? 0, first = (job: Job) => +close.has(job.model);
        this.waiting.sort((a, b) => +(a.phase !== undefined) - +(b.phase !== undefined) || first(b) - first(a) || share(b) - share(a));
        for (const job of this.waiting.splice(0, this.loads - this.loading)) {
            const entry = this.files.models.trees[job.model], url = job.phase === undefined ? entry?.summer : entry?.crowns[job.phase];
            this.loading++;
            void (url ? this.files.load(url) : Promise.resolve(undefined)).then(models => this.land(job, models?.[0]));
        }
    }

    /** Keeps a file's model; a file that failed is asked for again by the next `ask`. */
    private land(job: Job, model: Packed | undefined): void {
        this.loading--;
        if (!model) {
            this.asked.delete(keyOf(job));
            this.failed.add(keyOf(job));
            return this.post();
        }
        if (job.phase === undefined) this.leafy.set(job.model, model);
        else this.crowns.set(keyOf(job), model);
        this.stale = true;
        this.ask();
    }

    /**
     * Gives each species that has its summer crown its models: a phase that needs a crown of its own
     * draws it once loaded, else the summer crown in the phase's colours. A crown of its own that differs
     * only in colour draws the summer crown's impostor. Crowns of phases that the date lacks leave the pools.
     */
    private install(): void {
        // Crowns of their own take their instance models in entry and phase order once every species in reach
        // has its summer crown, so the indexes never hang on which file arrives first.
        const settled = this.builds().every(build => this.leafy.has(build.split('|')[0]) || this.failed.has(build.split('|')[0]));
        for (const [key, entry] of this.entries) {
            const [model, evergreen, tie] = key.split('|'), leafy = this.leafy.get(model), tones = this.files.models.trees[model]?.tones ?? [];
            if (!leafy) continue;
            const spec = ({ tiers, shape }: Packed): ModelSpec => ({
                tiers: tiers.map((mesh, t) => ({ mesh, until: TIERS_M[t] })), shape, impostor: { until: TREE_REACH_M }, tie: Number(tie), sized: TREE_SIZE_M,
            });
            const own = SEASONS.map((_, phase) => {
                if (evergreen === '1' || !this.phases.has(phase) || !this.ownIn(model, phase)) return undefined;
                if (settled) entry.own[phase] ??= this.instances.add({ tiers: [] });
                return this.crowns.get(`${model}|${phase}`);
            });
            entry.own.forEach((index, phase) => index !== undefined && this.instances.replace(index, own[phase] ? spec(own[phase]) : { tiers: [] }));
            this.instances.replace(entry.index, {
                ...spec(leafy), phases: evergreen === '1' ? undefined : SEASONS.map((_, phase) => {
                    const tone = tones[phase] ?? undefined, model = own[phase] && entry.own[phase];
                    return model === undefined ? { model: entry.index, tone } : { model, impostor: tone ? entry.index : model, tone };
                }),
            });
        }
    }

    /** The builds of the entries of these regions, by `model|evergreen`. */
    private builds(regions = this.wanted.reach): string[] {
        let builds = this.buildsOf.get(regions);
        if (!builds) this.buildsOf.set(regions, (builds = [...new Set([...this.entries].filter(([, entry]) => meets(entry.regions, regions)).map(([key]) => key.split('|').slice(0, 2).join('|')))]));
        return builds;
    }
}

const keyOf = ({ model, phase }: Job) => (phase === undefined ? model : `${model}|${phase}`);
