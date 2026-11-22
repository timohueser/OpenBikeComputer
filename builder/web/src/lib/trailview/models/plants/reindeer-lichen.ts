import type { Plant } from '../plant-library';
import { cushion, merge, ramp, random, type Ramp, type Tuft } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** Reindeer lichen: grey with a green cast while wet in spring and autumn, cream-white when dry in summer. */
const LICHEN: Record<Season, Ramp> = {
    winter: ['#7e8278', '#aeb0a2', '#d2d2c4'],
    spring: ['#7c8474', '#aab09a', '#cfd2bc'],
    summer: ['#a29c86', '#d2ccb4', '#f0ead4'],
    autumn: ['#828472', '#b2b29a', '#d6d4bc'],
};

/**
 * Reindeer lichen: four round puffs grown into each other, as the cushions of Cladonia stellaris, and two small
 * ones in the gaps at their rim. The heap is about five times as wide as tall. 168 triangles.
 */
export default {
    name: 'Reindeer lichen',
    latin: 'Cladonia rangiferina, Cladonia stellaris',
    build: (season, seed) => {
        const rand = random(seed), colours = ramp(LICHEN[season]);
        const turn = rand() * 2 * Math.PI;
        const puff = (k: number, of: number, d: number, r: number, h: number): Tuft => {
            const a = turn + ((k + 0.3 * rand()) / of) * 2 * Math.PI;
            return [d * Math.cos(a), d * Math.sin(a), r * (1 + 0.25 * rand()), h * (1 + 0.3 * rand())];
        };
        const round = [0, 1, 2, 3].map(k => cushion(puff(k, 4, 0.2, 0.26, 0.2), 10, colours, 0.45, seed + k));
        const small = [0, 1].map(k => cushion(puff(2 * k + 0.5, 4, 0.43, 0.17, 0.11), 8, colours, 0.45, seed + 4 + k));
        return merge([...round, ...small]);
    },
    ground: { on: ['moss', 'bare', 'shrub'], height: [0.05, 0.1], share: 0.2, own: true, clumped: true },
} satisfies Plant;
