// A cross on a summit or by the way: a post and a crossbar of square timber, steel or stone, 1 tall,
// base at the origin, z up, the crossbar along x.
import { rgb } from '../structures/mesh';
import { Model } from './flat';

export const MATERIALS: Record<string, number> = { wood: rgb('#6b4a2e'), metal: rgb('#c9ccc8'), stone: rgb('#a29f96') };
/** The `material` values that draw as one of MATERIALS. */
export const ALIASES: Record<string, string> = { steel: 'metal', iron: 'metal', concrete: 'stone' };
/** The post's side, the crossbar's height and half its width, as shares of the height. */
const SIDE = 0.06, BAR = 0.72, ARM = 0.27;

export function cross(material: string): Model {
    const colour = MATERIALS[material];
    return new Model().box([0, 0, 0.5 - 0.05], [SIDE, SIDE, 1.1], colour).box([0, 0, BAR], [2 * ARM, SIDE, SIDE], colour);
}
