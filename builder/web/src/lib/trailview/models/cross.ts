// Crosses by the way and on summits, of timber, steel or stone: 1 tall, base at the origin, z up, the crossbar along x.
import { rgb } from '../structures/mesh';
import { fieldstone } from './amenities';
import { Model } from './flat';

export const MATERIALS: Record<string, number> = { wood: rgb('#6b4a2e'), metal: rgb('#c9ccc8'), stone: rgb('#a29f96') };
const ROOF = rgb('#4f3622');
/** The `material` values that draw as one of MATERIALS. */
export const ALIASES: Record<string, string> = { steel: 'metal', iron: 'metal', concrete: 'stone' };
/** The post's side, the crossbar's height and half its width, as shares of the height. */
const SIDE = 0.06, BAR = 0.72, ARM = 0.27;
const STONE = rgb('#8f8b82'), CAIRN = rgb('#a39f95'), IRON = rgb('#3a3f42'), TIN = rgb('#aeb3b3');

/**
 * A cross by the way: of square timber under two roof boards, of slim steel with rays at its middle, or of stone that
 * widens to its ends on a stepped foot. Each stands on a block of stone and reaches ARM to each side.
 */
export function cross(material: string): Model {
    const colour = MATERIALS[material], m = new Model();
    if (material === 'metal') {
        const side = 0.035;
        m.box([0, 0, 0.45], [side, side, 1.1], colour).box([0, 0, BAR], [2 * ARM, side, side], colour).box([0, 0, 0.03], [0.14, 0.14, 0.06], STONE);
        // Four short rays between the arms.
        for (const [x, z] of [[1, 1], [-1, 1], [-1, -1], [1, -1]]) m.beam([0.03 * x, 0, BAR + 0.03 * z], [0.1 * x, 0, BAR + 0.1 * z], 0.02, IRON, { capped: true });
        return m;
    }
    if (material === 'stone') return stone(m, colour).box([0, 0, 0], [0.3, 0.22, 0.1], STONE).box([0, 0, 0.075], [0.2, 0.15, 0.05], colour);
    m.box([0, 0, 0.45], [SIDE, SIDE, 1.1], colour).box([0, 0, BAR], [2 * ARM, SIDE, SIDE], colour).box([0, 0, 0.025], [0.16, 0.16, 0.05], STONE);
    // Two boards from the ends of the crossbar to a ridge over the post's top.
    for (const x of [-1, 1]) m.prism([[x * ARM, BAR + 0.03], [0, 0.97], [0, 1], [x * ARM, BAR + 0.06]], -0.055, 0.055, ROOF);
    return m;
}

/** A stone cross: the post and the arms widen from the middle to their ends. */
function stone(m: Model, colour: number): Model {
    m.prism([[-0.05, 0.1], [0.05, 0.1], [0.035, BAR], [-0.035, BAR]], -0.04, 0.04, colour).prism([[-0.035, BAR], [0.035, BAR], [0.055, 1], [-0.055, 1]], -0.04, 0.04, colour);
    for (const x of [-1, 1]) m.prism([[0, BAR - 0.035], [x * ARM, BAR - 0.055], [x * ARM, BAR + 0.055], [0, BAR + 0.035]], -0.04, 0.04, colour);
    return m;
}

/**
 * A cross on a summit, in a cairn of stones with a box for the summit book: of heavy timber with braces under and over
 * the crossbar and tin caps on its ends, of steel as a frame of two bars with rungs between them, or of stone.
 */
export function summitCross(material: string): Model {
    const colour = MATERIALS[material], m = new Model();
    if (material === 'metal') {
        // Two bars with rungs between them, for the post and for the crossbar.
        for (const x of [-0.03, 0.03]) m.box([x, 0, 0.45], [0.016, 0.03, 1.1], colour);
        for (const z of [-0.03, 0.03]) m.box([0, 0, BAR + z], [2 * ARM, 0.03, 0.016], colour);
        for (let z = 0.14; z < 1; z += 0.12) m.box([0, 0, z], [0.06, 0.02, 0.012], colour);
        for (let x = 0.09; x < ARM; x += 0.09) for (const side of [-1, 1]) m.box([side * x, 0, BAR], [0.012, 0.02, 0.06], colour);
    } else if (material === 'stone') stone(m, colour);
    else {
        const side = 0.075;
        m.box([0, 0, 0.45], [side, side, 1.1], colour).box([0, 0, BAR], [2 * ARM, side, side], colour);
        for (const x of [-1, 1]) for (const z of [-1, 1]) m.beam([x * 0.15, 0, BAR + z * 0.02], [x * 0.02, 0, BAR + z * 0.15], 0.04, colour, { capped: true });
        for (const [x, z] of [[-ARM, BAR], [ARM, BAR], [0, 1]]) m.box([x, 0, z], [x ? 0.02 : side + 0.02, side + 0.02, x ? side + 0.02 : 0.02], TIN);
    }
    // The cairn about the foot, and the box of the summit book on the post.
    for (let k = 0; k < 6; k++) {
        const a = (2 * Math.PI * k) / 6 + 0.3;
        m.add(fieldstone(k, 0.1 + 0.03 * (k % 3), 0.07 + 0.02 * (k % 2), k % 2 ? STONE : CAIRN).place(a, [0.085 * Math.cos(a), 0.085 * Math.sin(a), 0]));
    }
    return m.box([0, -0.055, 0.3], [0.08, 0.04, 0.06], TIN);
}
