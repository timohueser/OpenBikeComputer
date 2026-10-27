import type { Plant } from '../plant-library';
import { random, tufts, type Ramp } from '../plant-shape';
import type { Season } from '../../rules/regions';
import { cushionMat } from './reindeer-lichen';

/** Golden olive under its hoary hair points, greener in the wet seasons, greyer in winter. */
const MOSS: Record<Season, Ramp> = {
    winter: ['#5e6248', '#888c6c', '#aaac92'],
    spring: ['#56683a', '#8a9c5c', '#b0c080'],
    summer: ['#525c30', '#7e8a4c', '#a4ac6a'],
    autumn: ['#5c6a3e', '#8c9864', '#b2ba8a'],
};

/**
 * Woolly fringe-moss: a lumpy carpet of eleven small, flat cushions run together, about ten times
 * as wide as tall, a few spots pale with hair points. 264 triangles.
 */
export default {
    name: 'Woolly fringe-moss',
    latin: 'Racomitrium lanuginosum',
    build: (season, seed) => {
        const rand = random(seed);
        return cushionMat(tufts([0, 0, 0.45, 1], 10, 0.5, [0.4, 0.36, 0.6], [0.35, 0.14, 0.4], rand), MOSS[season], ['#d4d6cc', 0.05], 1, seed, rand);
    },
    ground: { on: ['moss', 'bare'], height: [0.05, 0.2], share: 0.6, own: true },
} satisfies Plant;
