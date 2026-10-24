import type { Instances } from '../instances/instances';
import { seasonOf, type Season } from '../rules/regions';
import type { BuiltModel } from './model-worker';

/** Trees stand within this many metres of the eye: the stamps of main roads reach 4 km. */
export const TREE_REACH_M = 4500;
/** The mesh tiers of a tree: the prototype's three levels of detail, and where each ends; impostors follow to the reach. */
const TIERS_M = [30, 150, 700];

/**
 * The tree models of the date's season (model-worker.ts), one build per species for the forest, the
 * shrubs and the orchards. A species that a pack keeps green all year builds in summer. A model with a
 * tie to the ground's hue (draw.wgsl), such as a tree that stands as a shrub, is an instance model of
 * its own on the same meshes.
 */
export class TreeModels {
    /** Instance models by `model|evergreen|tie`. */
    private readonly indexes = new Map<string, number>();
    private readonly worker = new Worker(new URL('./model-worker.ts', import.meta.url), { type: 'module' });
    private season?: Season;
    private building = 0;
    private next = 0;
    private rebuilt = false;
    /** Whether models arrived since the last update. */
    newContent = false;

    constructor(private readonly instances: Instances, wake: () => void) {
        this.worker.onmessage = ({ data }: MessageEvent<{ id: number; built?: BuiltModel[]; error?: string }>) => {
            this.building--;
            if (data.error || !data.built) return void console.warn(`Tree models: ${data.error}`);
            if (data.id !== this.next - 1) return;
            const built = new Map(this.builds().map((name, i) => [name, data.built![i]]));
            for (const [key, index] of this.indexes) {
                const [model, evergreen, tie] = key.split('|');
                const { tiers, crown } = built.get(`${model}|${evergreen}`)!;
                this.instances.replace(index, { tiers: tiers.map((mesh, t) => ({ mesh, until: TIERS_M[t] })), impostor: { from: 1, until: TREE_REACH_M }, crown, tie: Number(tie) });
            }
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
        if (!this.indexes.has(key)) {
            const index = this.instances.add({ tiers: [] });
            if (index === undefined) return undefined;
            this.indexes.set(key, index);
        }
        return this.indexes.get(key);
    }

    /** Builds the models of the date's season. */
    update(date: string): void {
        this.newContent = this.rebuilt;
        this.rebuilt = false;
        const season = seasonOf(date);
        if (season === this.season || !this.indexes.size) return;
        this.season = season;
        this.building++;
        this.worker.postMessage({ id: this.next++, models: this.builds().map(name => name.split('|')).map(([model, evergreen]) => ({ model, season: evergreen === '1' ? 'summer' : season })) });
    }

    /** The builds, by `model|evergreen`. */
    private builds(): string[] {
        return [...new Set([...this.indexes.keys()].map(key => key.split('|').slice(0, 2).join('|')))];
    }
}
