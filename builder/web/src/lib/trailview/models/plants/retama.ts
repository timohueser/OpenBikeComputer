import type { Plant } from '../plant-library';
import { cushion, domed, hash, hex, merge, onDome, ramp, random, sheet, spike, UP, v3, type Geometry, type Ramp, type Tuft } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** Retama twigs: grey olive, browner in winter. */
const RETAMA: Record<Season, Ramp> = {
    winter: ['#454a38', '#767c5c', '#a4a684'],
    spring: ['#434e3a', '#768662', '#a8b48a'],
    summer: ['#434c3a', '#748260', '#a4b086'],
    autumn: ['#434c3a', '#748260', '#a2ae86'],
};

/** White flowers in late spring, black pods on a share of the twig tips in summer. */
const FLOWER = hex('#f8f2ee'), PINK = hex('#f0c8d0'), POD = hex('#4a4030'), PODS = 0.25;

/** The places of twigs on the dome; in flower every second one stands, in a wand of flowers. */
const TWIGS = 80;

/**
 * Teide broom: an almost hemispherical bush of rush-thin upright twigs. A grey-olive dome carries the mass, and the
 * twigs stand out of it all over, steeper than the dome, so they make the outline. In flower every second twig
 * stands, its upper half in a wand of white flowers, a few wands pink, and the dome pales a little.
 */
function retama(season: Season, seed: number, rand: () => number): Geometry {
    const [deep, mid, tip] = ramp(RETAMA[season]);
    const bloom = season === 'spring';
    const dome: Tuft = [0, 0, 0.82, 0.8];
    // Flowers deep in the bush pale its shade in bloom, still grey-olive.
    const inside = deep.clone().lerp(mid, 0.3).lerp(hex('#c8c4a8'), bloom ? 0.3 : 0);
    const light = domed(v3(), v3(1, 1, 1), 0.3);
    const cards = sheet();
    for (let j = 0; j < TWIGS; j++) {
        const [at, out] = onDome(dome, j, TWIGS, 1.42);
        const dir = out.clone().addScaledVector(UP, 0.9).add(v3(rand() - 0.5, rand() - 0.5, 0).multiplyScalar(0.25)).normalize();
        const end = at.clone().addScaledVector(dir, 0.34 + 0.16 * rand());
        const twig = mid.clone().lerp(tip, 0.2 + 0.6 * rand());
        if (season === 'summer' && hash(at, seed) < PODS) twig.lerp(POD, 0.7);
        if (bloom && j % 2) continue;
        const foot = at.clone().addScaledVector(dir, -0.12);
        spike(cards, foot.setZ(Math.max(foot.z, 0)), end, 0.03, [inside, twig], light);
        if (bloom) {
            const petal = hash(at, seed) < 0.2 ? PINK : FLOWER;
            spike(cards, at.clone().lerp(end, 0.3), end.clone().addScaledVector(dir, 0.04), 0.048, [petal.clone().lerp(twig, 0.35), petal], light);
        }
    }
    return merge([cushion(dome, 10, [deep.clone().multiplyScalar(0.8), inside, inside.clone().lerp(mid, 0.4)], 0.3, seed), cards.build()]);
}

export default {
    name: 'Retama',
    latin: 'Spartocytisus supranubius',
    build: (season, seed) => retama(season, seed, random(seed)),
    ground: { on: ['shrub', 'bare'], height: [1, 2], share: 0.1, own: true },
} satisfies Plant;
