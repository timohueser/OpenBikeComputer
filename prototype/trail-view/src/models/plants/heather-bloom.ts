import type { Plant } from '../library';
import { random } from '../shape';
import { heather } from './heather';

export default {
  name: 'Heather, flowering',
  latin: 'Calluna vulgaris, Erica cinerea',
  build: (season, seed) => heather(season, true, seed, random(seed)),
  ground: { on: ['shrub', 'moss'], height: [0.2, 0.4], share: 0.15, habitat: 'moor' },
} satisfies Plant;
