import type { Plant } from '../plant-library';
import { cushion, domed, leaf, merge, onDome, ramp, random, sheet, tufts, UP, v3, type Geometry, type Ramp } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** Kermes oak, lentisc and rosemary: evergreen, a little duller in winter. */
const GARRIGUE: Record<Season, Ramp> = {
    winter: ['#2a3322', '#41502f', '#66744a'],
    spring: ['#26381e', '#41602c', '#7a9a48'],
    summer: ['#24331d', '#3b5229', '#6a8040'],
    autumn: ['#283420', '#3f5229', '#6c7c42'],
};

/**
 * Kermes oak, lentisc and rosemary in one evergreen cushion: a round dome with four smaller ones grown into it,
 * a few of them lit by the sun, and leafy twig tips that stand out of the domes all over. 190 triangles.
 */
function garrigue(season: Season, seed: number, rand: () => number): Geometry {
    const [deep, mid, tip] = ramp(GARRIGUE[season]);
    const cards = sheet();
    const mounds = tufts([0, 0, 0.56, 0.9], 4, 0.8, [0.5, 0.36, 0.5], [0.1, 0.08, 0.16], rand);
    const parts = mounds.map((mound, k) => cushion(mound, k ? 6 : 8, [deep.clone().multiplyScalar(0.75), deep.clone().lerp(mid, 0.5), mid.clone().lerp(tip, 0.3)], 0.3, seed + k));
    mounds.forEach((mound, k) => {
        const [x, y, r, h] = mound, light = domed(v3(x, y, 0), v3(r, r, h), 0.3);
        const tips = k ? 8 : 15, turn = rand() * 2 * Math.PI, sun = rand() < 0.3 ? 0.6 : 0.2 * rand();
        for (let j = 0; j < tips; j++) {
            const [at, out] = onDome(mound, j, tips, 1.35, turn);
            const body = mid.clone().lerp(tip, sun + 0.3 * rand());
            leaf(cards, at, out.clone().addScaledVector(UP, 0.7).normalize(), 0.28 + 0.08 * rand(), 0.2, 0.5, [deep.clone().lerp(mid, 0.5), body, body.clone().lerp(tip, 0.5)], light);
        }
    });
    return merge([...parts, cards.build()]);
}

export default {
    name: 'Garrigue',
    latin: 'Quercus coccifera, Pistacia lentiscus, Salvia rosmarinus',
    build: (season, seed) => garrigue(season, seed, random(seed)),
    ground: { on: ['shrub', 'grass', 'bare'], height: [0.35, 0.8], share: 0.16, own: true },
} satisfies Plant;
