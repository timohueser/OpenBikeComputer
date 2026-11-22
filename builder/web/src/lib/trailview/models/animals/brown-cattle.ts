import { rgb } from '../../structures/mesh';
import type { Animal } from '../animals';
import { cattle } from './body/cattle';

export default {
    name: 'Brown cattle',
    latin: 'Bos taurus',
    height: 1.4,
    coats: ['#9a7a5c', '#8e4a24', '#ae8250'].map(rgb),
    poses: 3,
    build: (s, coat, pose) => cattle(s, pose, { coat }),
} satisfies Animal<number>;
