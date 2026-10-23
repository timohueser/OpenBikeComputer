// A cross on a summit or by the way: a post and a crossbar of square timber, steel or stone, base at the
// origin, z up, the crossbar along x.
import { hex, Model } from './shape';

const MATERIALS: Record<string, number> = { wood: hex('#6b4a2e'), metal: hex('#c9ccc8'), steel: hex('#c9ccc8'), stone: hex('#a29f96'), concrete: hex('#b3b0a8') };
/** The post's side as a share of the cross's height, the crossbar's height and half its width. */
const SIDE = 0.06, BAR = 0.72, ARM = 0.27;

export function cross(height: number, material = 'wood'): Model {
    const colour = MATERIALS[material] ?? MATERIALS.wood, side = SIDE * height;
    return new Model().box([0, 0, height / 2 - 0.25], [side, side, height + 0.5], colour).box([0, 0, BAR * height], [2 * ARM * height, side, side], colour);
}
