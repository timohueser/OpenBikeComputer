import type { Instances } from '../instances/instances';
import type { ModelFiles } from '../instances/model-files';
import { Points, POINT_WORDS } from '../instances/points';
import { KINDS } from '../kinds';
import { poolWords } from './light-pools';

/** Points of the kinds on the GPU at most. */
const CAPACITY = 2 ** 15;

/**
 * The models on the surface of the kinds: their models in the instances, in the order of POINT_MODELS, from the objects
 * file (models/model-file.ts), and the points of the drawn nodes in one pool.
 */
export class KindPoints {
    private readonly pool: Points;
    private readonly first?: number;
    private lists: Uint32Array[] = [];
    /** The light of each model, in the order of POINT_MODELS. */
    private readonly lightOf = [...KINDS].flatMap(([, kind]) => Object.keys(kind.models ?? {}).map(model => kind.lights?.[model]));
    /** The pools of light of the points, as light-pools.ts takes them. */
    lights = new Uint32Array(0);
    /** Whether the objects file loads, and whether its models arrived since the last update. */
    private loading = true;
    private arrived = false;
    /** Whether the last update installed the models. */
    newContent = false;

    /** `wake` asks for a frame when the models arrive. */
    constructor(instances: Instances, files: ModelFiles, wake: () => void) {
        const { objects } = files.models, first = instances.add(...objects.points.map(() => ({ tiers: [] })));
        this.first = first;
        this.pool = new Points(instances, CAPACITY);
        void files.load(objects.file).then(models => {
            this.loading = false;
            if (first === undefined || !models) return;
            objects.points.forEach(({ until, casts }, k) => {
                const { tiers, shape } = models[objects.rocks + k];
                instances.replace(first + k, { tiers: tiers.map((mesh, t) => ({ mesh, until: until[t] })), shape, casts });
            });
            this.arrived = true;
            wake();
        });
    }

    get idle(): boolean {
        return !this.loading;
    }

    /** The points of the drawn nodes' kinds, each list as `packPoints` packs it; a new set of lists replaces the pool's points. */
    update(lists: Uint32Array[]): void {
        this.newContent = this.arrived;
        this.arrived = false;
        const first = this.first;
        if (first === undefined || (lists.length === this.lists.length && lists.every((list, i) => list === this.lists[i]))) return;
        this.lists = lists;
        const out = new Uint32Array(lists.reduce((n, list) => n + list.length, 0));
        let at = 0;
        for (const list of lists) {
            out.set(list, at);
            at += list.length;
        }
        // A point's model is its number in POINT_MODELS: the instances number them from `first`.
        const lights: number[] = [], ints = new Int32Array(out.buffer);
        for (let i = 2; i < out.length; i += POINT_WORDS) {
            // The pool draws CAPACITY points at most: a point that it leaves out sheds no light either.
            const light = i < CAPACITY * POINT_WORDS ? this.lightOf[out[i] & 0xffff] : undefined;
            if (light) lights.push(...poolWords(ints[i - 2], ints[i - 1], (out[i] >> 16) & 255, light));
            out[i] = (out[i] & 0xffff0000) | ((out[i] & 0xffff) + first);
        }
        this.lights = new Uint32Array(lights);
        this.pool.write(out);
    }
}
