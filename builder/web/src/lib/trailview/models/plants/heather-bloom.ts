import type { Plant } from '../plant-library';
import { random } from '../plant-shape';
import { heather } from './heather';

export default {
    name: 'Heather, flowering',
    latin: 'Calluna vulgaris, Erica cinerea',
    build: (season, seed) => heather(season, true, seed, random(seed)),
    ground: { on: ['#moor'], height: [0.2, 0.4], share: 0.15 },
} satisfies Plant;
