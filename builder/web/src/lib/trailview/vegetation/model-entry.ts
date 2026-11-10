// A built tree model as an entry of the device cache. The tree version hashes this module with the model worker, so a
// change of the layout makes another cache.
import { SEASONS } from '../rules/regions';
import type { BuiltLeafy, BuiltModel } from './model-worker';

/** Numbers in the head per phase of a summer crown: whether the phase draws a crown of its own, whether it has a colour matrix, and the matrix. */
const PHASE_WORDS = 11;

/** A built model as the parts of an entry: a float64 head (the crown's lowest point, the tiers' trunks and, for a summer crown, its phases), then the arrays of each tier. */
export function partsOf(model: BuiltModel | BuiltLeafy): Uint32Array[] {
    const { tiers, crown } = model, leafy = 'tones' in model ? model : undefined;
    const head = [crown, tiers.length, ...tiers.map(({ trunk }) => trunk), leafy ? 1 : 0];
    if (leafy) SEASONS.forEach((_, phase) => head.push(+!!leafy.own[phase], leafy.tones[phase] ? 1 : 0, ...(leafy.tones[phase] ?? Array(9).fill(0))));
    return [new Uint32Array(Float64Array.from(head).buffer), ...tiers.flatMap(({ position, normal, color, index }) => [position, normal, color, index].map(array => new Uint32Array(array.buffer, array.byteOffset, array.length)))];
}

/** The model of an entry; it throws where the entry does not hold one. */
export function modelOf([words, ...arrays]: Uint32Array[]): BuiltModel | BuiltLeafy {
    const head = new Float64Array(words.buffer), count = head[1], at = 2 + count;
    if (!Number.isInteger(count) || arrays.length !== 4 * count || head.length !== at + 1 + (head[at] ? PHASE_WORDS * SEASONS.length : 0)) throw new Error('not a tree model');
    const tiers = [...head.subarray(2, at)].map((trunk, t) => {
        const [position, normal, color] = arrays.slice(4 * t, 4 * t + 3).map(array => new Float32Array(array.buffer));
        return { position, normal, color, index: arrays[4 * t + 3], trunk };
    });
    if (!head[at]) return { tiers, crown: head[0] };
    const phases = SEASONS.map((_, phase) => head.subarray(at + 1 + PHASE_WORDS * phase, at + 1 + PHASE_WORDS * (phase + 1)));
    return { tiers, crown: head[0], own: phases.map(words => words[0] === 1), tones: phases.map(words => (words[1] ? [...words.subarray(2)] : undefined)) };
}
