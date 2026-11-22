import type { Plant } from '../plant-library';
import { cushion, domed, hash, hex, merge, onDome, ramp, random, sheet, spike, tufts, UP, v3, type Geometry, type Ramp } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** Dark green spines, browner in winter. */
const SPINES: Record<Season, Ramp> = {
    winter: ['#28341e', '#40502c', '#62703e'],
    spring: ['#243a1e', '#3a582a', '#5e7c3e'],
    summer: ['#24381e', '#38562a', '#5a783c'],
    autumn: ['#26381e', '#3a542a', '#5c763c'],
};

/** The share of spray tips in flower: it blooms from late summer into autumn, with heather. */
const BLOOM: Record<Season, number> = { winter: 0.06, spring: 0.04, summer: 0.45, autumn: 0.5 };
const FLOWER = hex('#f2c41c');
/** The deeper yellow at a flower's foot. */
const AMBER = hex('#c08a10');

/**
 * Western gorse: a low, dense cushion of five domes grown into each other, bristling with stiff spines. In bloom
 * short, stout yellow flowers stand among the spines, most on the upper side. 216 triangles.
 */
function westernGorse(season: Season, seed: number, rand: () => number): Geometry {
    const [deep, mid, tip] = ramp(SPINES[season]);
    const cards = sheet();
    const mounds = tufts([0, 0, 0.55, 0.8], 4, 0.7, [0.62, 0.42, 0.5], [0.15, 0.1, 0.15], rand);
    const parts = mounds.map((mound, k) => cushion(mound, k ? 6 : 8, [deep.clone().multiplyScalar(0.7), deep, deep.clone().lerp(mid, 0.7)], 0.3, seed + k));
    mounds.forEach((mound, k) => {
        const [x, y, r, h] = mound, light = domed(v3(x, y, 0), v3(r, r, h), 0.3);
        const spines = k ? 7 : 12, turn = rand() * 2 * Math.PI;
        for (let j = 0; j < spines; j++) {
            const [foot, out] = onDome(mound, j, spines, 1.3, turn);
            const dir = out.clone().addScaledVector(UP, 0.25).normalize(), length = 0.28 + 0.12 * rand();
            const spray = mid.clone().lerp(tip, 0.3 + 0.6 * rand());
            // Flowers are picked by hash, so a bush keeps its shape in every season.
            if (hash(foot, seed) < BLOOM[season] * (out.z > 0.5 ? 1.3 : 0.6)) spike(cards, foot, foot.clone().addScaledVector(dir, 0.2), 0.09, [FLOWER.clone().lerp(AMBER, 0.6), FLOWER], light);
            else spike(cards, foot, foot.clone().addScaledVector(dir, length), 0.05, [deep.clone().lerp(mid, 0.5), spray], light);
        }
    });
    return merge([...parts, cards.build()]);
}

export default {
    name: 'Western gorse',
    latin: 'Ulex gallii',
    build: (season, seed) => westernGorse(season, seed, random(seed)),
    ground: { on: ['shrub', 'moss'], height: [0.2, 0.6], share: 0.3, own: true },
} satisfies Plant;
