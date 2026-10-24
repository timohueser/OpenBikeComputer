import { dayOfYear } from '../ground/rules';
import type { Instances, ModelSpec } from '../instances/instances';
import { SEASONS } from '../rules/regions';
import type { BuiltModel, BuiltSpecies, ModelJob } from './model-worker';
import { phasesOn } from './phenology';

/** Trees stand within this many metres of the eye: the stamps of main roads reach 4 km. */
export const TREE_REACH_M = 4500;
/**
 * The mesh tiers of a tree: three levels of detail, and where each ends for a tree TREE_SIZE_M tall; a
 * tree of another height switches where it draws as large. Impostors follow to the reach.
 */
const TIERS_M = [60, 300, 700], TREE_SIZE_M = 25;

/** An instance model of a species, and the models of its phases that draw a crown of their own. */
interface Entry { index: number; own: (number | undefined)[] }

/**
 * The tree models (model-worker.ts), one build per species for the forest, the shrubs and the
 * orchards. A tree draws the summer crown, which its phase on the date (phenology.ts) colours, or a
 * crown of the phase's own, such as a bare one; the worker builds these for the phases that the date
 * has. A species that a pack keeps green all year stays in summer. A model with a tie to the ground's
 * hue (draw.wgsl), such as a tree that stands as a shrub, is an instance model of its own on the same
 * meshes.
 */
export class TreeModels {
    /** Instance models by `model|evergreen|tie`. */
    private readonly entries = new Map<string, Entry>();
    private readonly worker = new Worker(new URL('./model-worker.ts', import.meta.url), { type: 'module' });
    /** The phases of the last build, as a key, and the day of the trees, which waits for a build that runs. */
    private phases?: string;
    private day = 0;
    private building = 0;
    private next = 0;
    private rebuilt = false;
    /** Whether models arrived since the last update. */
    newContent = false;

    constructor(private readonly instances: Instances, wake: () => void) {
        this.worker.onmessage = ({ data }: MessageEvent<{ id: number; built?: BuiltSpecies[]; error?: string }>) => {
            this.building--;
            if (data.error || !data.built) return void console.warn(`Tree models: ${data.error}`);
            if (data.id !== this.next - 1) return;
            const built = new Map(this.builds().map((name, i) => [name, data.built![i]]));
            for (const [key, entry] of this.entries) {
                const [model, evergreen, tie] = key.split('|');
                const { leafy, own } = built.get(`${model}|${evergreen}`)!;
                const spec = (crown: BuiltModel): ModelSpec => ({
                    tiers: crown.tiers.map((mesh, t) => ({ mesh, until: TIERS_M[t] })), impostor: { from: 2, until: TREE_REACH_M }, crown: crown.crown, tie: Number(tie), sized: TREE_SIZE_M,
                });
                // A crown of its own that the date no longer has leaves the pools; its model stays for a later date.
                SEASONS.forEach((_, phase) => {
                    if (own[phase]) entry.own[phase] ??= this.instances.add({ tiers: [] });
                    if (entry.own[phase] !== undefined) this.instances.replace(entry.own[phase]!, own[phase] ? spec(own[phase]) : { tiers: [] });
                });
                this.instances.replace(entry.index, {
                    ...spec(leafy), phases: evergreen === '1' ? undefined : SEASONS.map((_, phase) => {
                        const tone = leafy.tones[phase], model = own[phase] && entry.own[phase];
                        // A crown of its own in colour only draws the summer crown's impostor.
                        return model === undefined ? { model: entry.index, tone } : { model, impostor: tone ? entry.index : model, tone };
                    }),
                });
            }
            this.instances.day = this.day;
            this.rebuilt = true;
            wake();
        };
    }

    get idle(): boolean {
        return this.building === 0;
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

    /** Builds the models of the phases that the date has; the trees take the date's phases when they arrive. */
    update(date: string): void {
        this.newContent = this.rebuilt;
        this.rebuilt = false;
        const day = dayOfYear(date);
        if ((day === this.day && this.phases !== undefined) || !this.entries.size) return;
        this.day = day;
        const phases = [...phasesOn(day)].sort();
        if (phases.join() !== this.phases) {
            this.phases = phases.join();
            this.building++;
            this.worker.postMessage({ id: this.next++, models: this.builds().map(name => name.split('|')).map(([model, evergreen]) => ({ model, evergreen: evergreen === '1', phases })) } satisfies ModelJob);
        } else if (!this.building) {
            // The same models serve the day: the trees change phase with no build.
            this.instances.day = day;
            this.newContent = true;
        }
    }

    /** The builds, by `model|evergreen`. */
    private builds(): string[] {
        return [...new Set([...this.entries.keys()].map(key => key.split('|').slice(0, 2).join('|')))];
    }
}
