import type { Plant } from '../plant-library';
import { byHeight, merge, paint, ramp, random, ribbon, type Geometry, type Ramp, type Vec } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** The grass's colours, root to tip, by season. */
const GRASS: Record<Season, Ramp> = {
    winter: ['#6e6a4c', '#9a9168', '#c2b88c'],
    spring: ['#4f7a35', '#7fae4a', '#b4d36e'],
    summer: ['#557a34', '#86a84a', '#c2c46a'],
    autumn: ['#6a7038', '#97935a', '#c4b47a'],
};

/**
 * Nine blades, each bent once at its middle: three stand in the middle and nod, and six shorter ones leave the
 * foot at a slant and bow over round them, so the tuft is a small fountain.
 */
function grass(season: Season, rand: () => number): Geometry {
    const colour = byHeight(ramp(GRASS[season]), 0.9);
    const parts: Geometry[] = [];
    for (let k = 0; k < 9; k++) {
        const inner = k < 3, a = (inner ? k / 3 : (k + 0.5) / 6) * 2 * Math.PI + rand() * 0.6;
        const [c, s] = [Math.cos(a), Math.sin(a)], half = (inner ? 0.5 : 0.42) * (1 + 0.25 * rand());
        const [rise, bow] = inner ? [0.05 + 0.2 * rand(), 0.35 + 0.25 * rand()] : [0.4 + 0.25 * rand(), 1.1 + 0.3 * rand()];
        const d = inner ? 0 : 0.05, reach = half * (Math.sin(rise) + Math.sin(bow));
        const middle: Vec = [(d + half * Math.sin(rise)) * c, (d + half * Math.sin(rise)) * s, half * Math.cos(rise)];
        const tip: Vec = [(d + reach) * c, (d + reach) * s, half * (Math.cos(rise) + Math.cos(bow))];
        const width = 0.1 + 0.03 * rand();
        parts.push(paint(ribbon([[d * c, d * s, 0], middle, tip], [-s, c, 0], (t) => width * (1 - t * t), 2), colour));
    }
    return merge(parts);
}

export default {
    name: 'Grass tuft',
    latin: 'Poaceae',
    build: (season, seed) => grass(season, random(seed)),
    ground: { on: ['grass', 'crop'], height: [0.25, 0.5], share: 1 },
} satisfies Plant;
