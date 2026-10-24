import type { Instances } from '../instances/instances';
import { Points, POINT_WORDS } from '../instances/points';
import { KINDS } from '../kinds';

/** Points of the kinds on the GPU at most. */
const CAPACITY = 2 ** 15;

/**
 * The models on the surface of the kinds: their models in the instances, in the order of POINT_MODELS,
 * and the points of the drawn nodes in one pool.
 */
export class KindPoints {
    private readonly pool: Points;
    private readonly first: number;
    private lists: Uint32Array[] = [];

    constructor(instances: Instances) {
        const specs = [...KINDS].flatMap(([, kind]) => Object.values(kind.models ?? {}));
        this.first = specs.length ? specs.map(spec => instances.add(spec()))[0] : 0;
        this.pool = new Points(instances, CAPACITY);
    }

    /** The points of the drawn nodes' kinds, each list as `packPoints` packs it; a new set of lists replaces the pool's points. */
    update(lists: Uint32Array[]): void {
        if (lists.length === this.lists.length && lists.every((list, i) => list === this.lists[i])) return;
        this.lists = lists;
        const out = new Uint32Array(lists.reduce((n, list) => n + list.length, 0));
        let at = 0;
        for (const list of lists) {
            out.set(list, at);
            at += list.length;
        }
        // A point's model is its number in POINT_MODELS: the instances number them from `first`.
        for (let i = 2; i < out.length; i += POINT_WORDS) out[i] = (out[i] & 0xffff0000) | ((out[i] & 0xffff) + this.first);
        this.pool.write(out);
    }
}
