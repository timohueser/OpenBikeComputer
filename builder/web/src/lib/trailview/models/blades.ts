// A patch of grass blades for the near sward: thin blades spread over a patch as wide as they are
// tall, each two segments, lit as the ground they grow from.
import type { Season } from '../rules/regions';
import { blade, byHeight, merge, paint, ramp, random, type Geometry, type Vec } from './plant-shape';
import { GRASS } from './plants/grass';

/** Blades in a patch, 1 tall: 2 triangles each. */
export function bladePatch(season: Season, blades = 24, seed = 3): Geometry {
    const rand = random(seed), colour = byHeight(ramp(GRASS[season]), 1);
    return merge(Array.from({ length: blades }, () => {
        const base: Vec = [rand() - 0.5, rand() - 0.5, 0];
        const length = 0.6 + 0.4 * rand();
        return paint(blade(base, rand() * 2 * Math.PI, length, 0.1 + 0.4 * rand(), 0.25, t => 0.035 * (1 - t), 2), colour);
    }));
}
