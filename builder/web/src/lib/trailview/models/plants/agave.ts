import type { Plant } from '../plant-library';
import { GOLDEN, hex, ramp, random, sheet, spike, spindle, UP, v3, type Geometry, type Ramp } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** The leaves: blue-grey with a waxy bloom, the same all year. */
const LEAVES: Ramp = ['#4a6068', '#7a949a', '#aac0c0'];

/** A rosette dies back to dull grey-brown once its pole has flowered. */
const SPENT: Ramp = ['#5a5848', '#827e68', '#a8a288'];

/** The pole and its flower clusters: green-yellow and in flower in summer, dead and brown otherwise. */
const POLE: Record<Season, [pole: string, clusters: string]> = {
    winter: ['#6a5a44', '#5a4a38'],
    spring: ['#7a8a4a', '#8a9a5a'],
    summer: ['#8a9a50', '#e8c838'],
    autumn: ['#7a6448', '#6a5440'],
};

/** Of the plant's shapes, the third carries a flower pole. */
export const POLE_SEED = 3;

/** The leaves of a rosette and the branches of a pole. */
const BLADES = 16, BRANCHES = 7;

/**
 * Agave: a rosette of sixteen thick sword leaves, the inner ones upright round the bud and the outer ones spread
 * out, each ending in a dark spine. A leaf is a solid three-sided point: a flat upper face, widest at its foot, on
 * a keel, so the feet make the plant's heart. A plant of the third shape carries a flower pole about four rosettes
 * tall with branches held out at the top, ending in flower clusters, in flower in summer and dead after, its
 * rosette spent with it. It builds itself with the rosette 1 tall, so the pole stands above the plant's height.
 * 48 triangles, 100 with the pole.
 */
function agave(season: Season, seed: number, rand: () => number): Geometry {
    const pole = seed === POLE_SEED;
    const [deep, mid, tip] = ramp(pole && season !== 'summer' ? SPENT : LEAVES);
    const spine = hex('#4a3a30');
    const cards = sheet();
    for (let k = 0; k < BLADES; k++) {
        const a = k * GOLDEN + 0.3 * rand();
        const s = Math.sqrt((k + 0.5) / BLADES);
        // Inner leaves stand; outer ones spread.
        const polar = 0.1 + 1.02 * s;
        const length = 1.04 - 0.2 * s + 0.1 * rand();
        const out = v3(Math.cos(a), Math.sin(a), 0), side = v3(-out.y, out.x, 0);
        const dir = out.clone().multiplyScalar(Math.sin(polar)).setZ(Math.cos(polar));
        // The leaf's upper face looks to the plant's axis and up.
        const face = v3().crossVectors(side, dir);
        const foot = out.clone().multiplyScalar(0.03).setZ(0.02);
        const [left, right] = [1, -1].map((e) => foot.clone().addScaledVector(side, e * (0.13 + 0.03 * s)).addScaledVector(face, 0.03));
        const keel = foot.clone().addScaledVector(face, -0.1), end = foot.clone().addScaledVector(dir, length);
        const body = mid.clone().lerp(tip, 0.5 * rand()), under = deep.clone().lerp(mid, 0.4);
        const up = face.clone().lerp(UP, 0.5).normalize(), down = out.clone().lerp(UP, 0.15).normalize();
        cards.add([left, right, end], [body, body, spine], (p) => (p === end ? up : up.clone().addScaledVector(side, p === left ? 0.5 : -0.5).normalize()));
        for (const edge of [left, right]) cards.add([edge, keel, end], [body, under, spine], (p) => (p === keel ? down : down.clone().addScaledVector(side, edge === left ? 0.7 : -0.7).normalize()));
    }
    if (pole) {
        const [stalk, cluster] = POLE[season].map(hex);
        const height = 4 + 0.8 * rand(), top = v3(0.1, 0, height);
        spike(cards, v3(0, 0, 0.3), top, 0.06, [stalk.clone().multiplyScalar(0.8), stalk]);
        for (let b = 0; b < BRANCHES; b++) {
            const a = b * GOLDEN + rand(), t = b / BRANCHES;
            const from = v3(0.1 * (0.72 + 0.26 * t), 0, height * (0.72 + 0.26 * t));
            const reach = 0.55 * (1 - 0.65 * t);
            const to = from.clone().add(v3(reach * Math.cos(a), reach * Math.sin(a), 0.14));
            cards.add([from.clone().setZ(from.z + 0.03), from.clone().setZ(from.z - 0.03), to], [stalk, stalk, stalk]);
            // A cluster: a low round head on the branch end.
            spindle(cards, to.clone().setZ(to.z - 0.03), to.clone().setZ(to.z + 0.16), 0.15, 3, [stalk, cluster]);
        }
    }
    return cards.build();
}

export default {
    name: 'Agave',
    latin: 'Agave americana',
    build: (season, seed) => agave(season, seed, random(seed)),
    ground: { on: ['shrub', 'bare'], height: [1, 2], share: 0.01, own: true },
    sized: true,
} satisfies Plant;
