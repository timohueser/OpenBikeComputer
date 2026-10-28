import type { Plant } from '../plant-library';
import { GOLDEN, hex, limb, merge, ramp, random, sheet, smooth, v3, type Geometry, type Ramp, type V } from '../plant-shape';
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

/**
 * Agave: a rosette of thick, stiff sword leaves, the inner ones upright and the outer ones spread
 * out, a few bent down at the tip. Each leaf is two triangles folded along its keel, so it reads
 * as thick. A plant of the third shape carries a flower pole about four rosettes tall with
 * branches held out flat at the top, ending in flower clusters, in flower in summer and dead
 * after, its rosette spent with it. It builds
 * itself with the rosette 1 tall, so the pole stands above the plant's height. 48 triangles, 140
 * with the pole.
 */
function agave(season: Season, seed: number, rand: () => number): Geometry {
    const pole = seed === POLE_SEED;
    const [deep, mid, tip] = ramp(pole && season !== 'summer' ? SPENT : LEAVES);
    const light = (p: V) => v3(p.x, p.y, 0.6).normalize();
    const cards = sheet();
    for (let k = 0; k < 24; k++) {
        const a = k * GOLDEN + 0.3 * rand();
        const s = Math.sqrt((k + 0.5) / 24);
        // Inner leaves stand; outer ones spread nearly flat.
        const polar = 0.2 + 1.15 * s;
        const length = 1.05 - 0.25 * s + 0.1 * rand();
        const out = v3(Math.cos(a), Math.sin(a), 0);
        const side = v3(-out.y, out.x, 0).multiplyScalar(0.07 + 0.03 * s);
        const end = out.clone().multiplyScalar(length * Math.sin(polar)).setZ(length * Math.cos(polar));
        // A few old leaves break and hang at the tip.
        if (rand() < 0.15 * s) end.z -= 0.25 * length;
        const keel = out.clone().multiplyScalar(0.05).setZ(0.05);
        const ridge = keel.clone().lerp(end, 0.4).add(v3(0, 0, 0.04));
        const [foot, body, top] = [deep, mid.clone().lerp(tip, 0.3 * rand()), tip.clone().lerp(mid, 0.3)];
        const base = keel.clone().setZ(0.02);
        cards.add([base.clone().add(side), ridge, end], [foot, body, top], light);
        cards.add([ridge, base.clone().sub(side), end], [body, foot, top], light);
    }
    const parts = [cards.build()];
    if (pole) {
        const [stalk, cluster] = POLE[season].map(hex);
        const height = 4 + 0.8 * rand();
        const shade = (p: V) => stalk.clone().multiplyScalar(0.8 + 0.2 * smooth(0, height, p.z));
        parts.push(limb([0, 0, 0.3], [0.1, 0, height], 0.05, shade, 0, 4));
        const heads = sheet();
        for (let b = 0; b < 8; b++) {
            const a = b * GOLDEN + rand();
            const from = v3(0.1 * (0.75 + 0.25 * (b / 8)), 0, height * (0.75 + 0.22 * (b / 8)));
            const reach = 0.5 * (1 - 0.6 * (b / 8));
            const to = from.clone().add(v3(reach * Math.cos(a), reach * Math.sin(a), 0.12));
            const across = v3(-Math.sin(a), Math.cos(a), 0).multiplyScalar(0.02);
            heads.add([from.clone().add(across), from.clone().sub(across), to], [stalk, stalk, stalk]);
            // A cluster: a flat cup of four triangles opening upwards.
            const ring = [0, 1, 2, 3].map((i) => to.clone().add(v3(0.12 * Math.cos(a + (i * Math.PI) / 2), 0.12 * Math.sin(a + (i * Math.PI) / 2), 0.1)));
            for (let i = 0; i < 4; i++) heads.add([to, ring[i], ring[(i + 1) % 4]], [stalk, cluster, cluster]);
        }
        parts.push(heads.build());
    }
    return merge(parts);
}

export default {
    name: 'Agave',
    latin: 'Agave americana',
    build: (season, seed) => agave(season, seed, random(seed)),
    ground: { on: ['shrub', 'bare'], height: [1, 2], share: 0.01, own: true },
    sized: true,
} satisfies Plant;
