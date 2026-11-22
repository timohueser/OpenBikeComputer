import type { Plant } from '../plant-library';
import { cushion, domed, hash, hex, leaf, merge, onDome, ramp, random, sheet, tufts, UP, v3, type Geometry, type Ramp } from '../plant-shape';
import type { Season } from '../../rules/regions';

const HEATHER: Record<Season, Ramp> = {
    winter: ['#3a3a26', '#5a5434', '#7a6a40'],
    spring: ['#34462b', '#5b7340', '#7d9150'],
    summer: ['#34462b', '#5b7340', '#86984f'],
    autumn: ['#3a4228', '#5f6838', '#8c6b40'],
};

/** Flowers in bloom on a clump of bell heather and ling, by season: colour and share. */
const HEATHER_BLOOM: Record<Season, [string, number]> = {
    winter: ['#7a5a3c', 0.3],
    spring: ['#7d9150', 0],
    summer: ['#9a5a8c', 0.8],
    autumn: ['#8a5a62', 0.5],
};

/**
 * A low mound of four round cushions grown into each other, set close with short upright sprays, each a pointed
 * blade. The sprays in flower, or out of bloom the rusty ones, take the flower's colour from the waist to the tip,
 * and the flowers tint the cushions' tops. 228 triangles.
 */
export function heather(season: Season, bloom: boolean, seed: number, rand: () => number): Geometry {
    const [deep, mid, tip] = ramp(HEATHER[season]);
    const [flower, share] = bloom ? HEATHER_BLOOM[season] : ['#7d6a44', 0.15];
    const [dots, bronze] = [hex(flower), hex('#6a5530')];
    const cards = sheet();
    const mounds = tufts([0, 0, 0.5, 0.7], 3, 0.8, [0.46, 0.36, 0.46], [0.1, 0.08, 0.12], rand);
    const parts = mounds.map((mound, k) => cushion(mound, k ? 6 : 8, [deep, deep.clone().lerp(mid, 0.6), mid.clone().lerp(dots, 0.5 * share)], 0.3, seed + k));
    mounds.forEach((mound, k) => {
        const [x, y, r, h] = mound, light = domed(v3(x, y, 0), v3(r, r, h), 0.3);
        const sprays = k ? 15 : 30, turn = rand() * 2 * Math.PI;
        for (let j = 0; j < sprays; j++) {
            const [foot, out] = onDome(mound, j, sprays, 1.45, turn);
            // A spray stands more upright than the dome's normal.
            const dir = out.clone().multiplyScalar(0.5).add(UP).normalize();
            const twig = mid.clone().lerp(tip, 0.3 + 0.5 * rand()).lerp(bronze, rand() < 0.35 ? 0.5 * rand() : 0);
            const flowering = hash(foot, seed) < share, strength = bloom ? 1 : 0.6;
            const colours = flowering ? [twig, twig.clone().lerp(dots, 0.85 * strength), twig.clone().lerp(dots, strength)] : [deep.clone().lerp(mid, 0.5), twig, twig.clone().lerp(tip, 0.4)];
            leaf(cards, foot, dir, 0.27 + 0.12 * rand(), 0.14, 0.55, colours, light);
        }
    });
    return merge([...parts, cards.build()]);
}

export default {
    name: 'Heather',
    latin: 'Calluna vulgaris',
    build: (season, seed) => heather(season, false, seed, random(seed)),
    ground: { on: ['#moor'], height: [0.2, 0.4], share: 0.4 },
} satisfies Plant;
