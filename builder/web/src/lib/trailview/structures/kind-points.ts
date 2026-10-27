import type { Instances } from '../instances/instances';
import { Points, POINT_WORDS } from '../instances/points';
import { KINDS } from '../kinds';
import { poolWords } from './light-pools';

/** Points of the kinds on the GPU at most. */
const CAPACITY = 2 ** 15;

/**
 * The models on the surface of the kinds: their models in the instances, in the order of POINT_MODELS,
 * and the points of the drawn nodes in one pool.
 */
export class KindPoints {
    private readonly pool: Points;
    private readonly first?: number;
    private lists: Uint32Array[] = [];
    /** The light of each model, in the order of POINT_MODELS. */
    private readonly lightOf = [...KINDS].flatMap(([, kind]) => Object.keys(kind.models ?? {}).map(model => kind.lights?.[model]));
    /** The pools of light of the points, as light-pools.ts takes them. */
    lights = new Uint32Array(0);

    constructor(instances: Instances) {
        const specs = [...KINDS].flatMap(([, kind]) => Object.values(kind.models ?? {}));
        this.first = instances.add(...specs.map(spec => spec()));
        this.pool = new Points(instances, CAPACITY);
    }

    /** The points of the drawn nodes' kinds, each list as `packPoints` packs it; a new set of lists replaces the pool's points. */
    update(lists: Uint32Array[]): void {
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
            const light = this.lightOf[out[i] & 0xffff];
            if (light) lights.push(...poolWords(ints[i - 2], ints[i - 1], light));
            out[i] = (out[i] & 0xffff0000) | ((out[i] & 0xffff) + first);
        }
        this.lights = new Uint32Array(lights);
        this.pool.write(out);
    }
}
