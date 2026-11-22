import type { Plant } from '../plant-library';
import { cushion, hex, merge, ramp, random, type Ramp } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** Golden olive, greener in the wet seasons, greyer in winter. */
const MOSS: Record<Season, Ramp> = {
    winter: ['#5e6248', '#888c6c', '#aaac92'],
    spring: ['#56683a', '#8a9c5c', '#b0c080'],
    summer: ['#525c30', '#7e8a4c', '#a4ac6a'],
    autumn: ['#5c6a3e', '#8c9864', '#b2ba8a'],
};

/** The hoary hair points that grey the top of a cushion. */
const HOAR = hex('#d4d6cc');

/**
 * Woolly fringe-moss: a carpet of four broad cushions grown together and five small ones in the gaps at their
 * rim, about nine times as wide as tall, each cushion hoary on its top.
 */
export default {
    name: 'Woolly fringe-moss',
    latin: 'Racomitrium lanuginosum',
    build: (season, seed) => {
        const rand = random(seed), [deep, mid, tip] = ramp(MOSS[season]);
        const colours = [deep, mid, tip.clone().lerp(HOAR, 0.45)];
        const turn = rand() * 2 * Math.PI;
        const broad = [0, 1, 2, 3].map(k => {
            const a = turn + (k / 4) * 2 * Math.PI, d = 0.3 + 0.1 * rand();
            return cushion([d * Math.cos(a), d * Math.sin(a), 0.38 + 0.1 * rand(), 0.16 + 0.05 * rand()], 12, colours, 0.3, seed + k);
        });
        const small = [0, 1, 2, 3, 4].map(k => {
            const a = turn + ((k + 0.5 + 0.4 * rand()) / 5) * 2 * Math.PI, d = 0.66 + 0.12 * rand();
            return cushion([d * Math.cos(a), d * Math.sin(a), 0.22 + 0.08 * rand(), 0.09 + 0.04 * rand()], 8, colours, 0.3, seed + 4 + k);
        });
        return merge([...broad, ...small]);
    },
    ground: { on: ['moss', 'bare'], height: [0.05, 0.2], share: 0.6, own: true },
} satisfies Plant;
