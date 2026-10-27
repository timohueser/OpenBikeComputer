import type { Plant } from '../plant-library';
import { domed, GOLDEN, hash, lathe, merge, paint, ramp, random, relight, sheet, smooth, soft, UP, v3, type Geometry, type Ramp } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** Oleander leaves: dark, narrow and leathery, the same all year. */
const LEAVES: Ramp = ['#1a2c1e', '#2c4830', '#5a7a52'];

/** Pink flowers on most bushes, white on one in five. */
const PINK: Ramp = ['#a83460', '#d8608c', '#f29ab8'];
const WHITE: Ramp = ['#b8b4ac', '#e8e4dc', '#faf8f2'];

/** The share of flower clusters open, from late spring to autumn. */
const BLOOM: Record<Season, number> = { winter: 0, spring: 0.4, summer: 0.95, autumn: 0.45 };

/** The bush's radius by height: narrow at the stool, widest high up, round on top. */
const OUTLINE: [number, number][] = [[0, 0.26], [0.3, 0.4], [0.65, 0.48], [0.85, 0.42], [0.97, 0.2], [1, 0]];
const radius = (z: number) => {
    const i = Math.max(1, OUTLINE.findIndex(([at]) => at >= z));
    const [[z0, r0], [z1, r1]] = [OUTLINE[i - 1], OUTLINE[i]];
    return r0 + ((r1 - r0) * (z - z0)) / (z1 - z0);
};

const LEAF_COUNT = 140;
const CLUSTERS = 28;

/**
 * Oleander: a dense vase of upright stems. A dark shell carries the mass; each leaf is one narrow
 * sliver lying up its outline and standing out at its tip, so the leaves make the outline. Flat
 * clusters of five flowers sit over the upper half. All of it lights as one dome. About 320
 * triangles in bloom.
 */
function oleander(season: Season, seed: number, rand: () => number): Geometry {
    const [deep, mid, tip] = ramp(LEAVES);
    const [shade, petal, lit] = ramp(hash(v3(seed, 1, 2), 5) < 0.2 ? WHITE : PINK);
    const light = domed(v3(0, 0, 0.4), v3(0.5, 0.5, 0.6), 0.2);
    const inside = deep.clone().lerp(mid, 0.25);
    // A point on the outline and its outward normal, at height z and angle a.
    const surface = (z: number, a: number) => {
        const out = v3(Math.cos(a), Math.sin(a), 0);
        const slope = (radius(Math.min(z + 0.02, 1)) - radius(Math.max(z - 0.02, 0))) / 0.04;
        return { at: out.clone().multiplyScalar(radius(z)).setZ(z), n: out.setZ(-slope).normalize() };
    };
    const cards = sheet();
    for (let j = 0; j < LEAF_COUNT; j++) {
        // More leaves high up, where the vase is wide.
        const { at, n } = surface(0.12 + 0.84 * ((j + 0.5) / LEAF_COUNT) ** 0.8, j * GOLDEN + 0.3 * rand());
        // Up the stems, a little out from the bush.
        const dir = UP.clone().addScaledVector(n, 0.35).add(v3(rand() - 0.5, rand() - 0.5, 0).multiplyScalar(0.4)).normalize();
        const length = 0.2 + 0.08 * rand();
        const top = at.clone().addScaledVector(dir, 0.55 * length);
        const foot = at.clone().addScaledVector(dir, -0.45 * length).addScaledVector(n, -0.03);
        const across = v3().crossVectors(dir, n).setLength(0.022 + 0.008 * rand());
        const leaf = mid.clone().lerp(tip, 0.15 + 0.5 * rand() * smooth(0.2, 0.9, at.z));
        cards.add([foot.clone().add(across), foot.clone().sub(across), top], [inside, inside, leaf], () => light(at));
    }
    // Drawn for every cluster in every season, so the bush keeps its shape the year round.
    for (let k = 0; k < CLUSTERS; k++) {
        const { at, n } = surface(0.5 + 0.4 * rand(), k * GOLDEN * 3 + rand());
        // On the stem tips, out over the leaves.
        const centre = at.clone().addScaledVector(n, 0.1).addScaledVector(UP, 0.04);
        const face = n.clone().addScaledVector(UP, 0.5).normalize();
        const side = v3().crossVectors(face, UP).normalize();
        const up = v3().crossVectors(side, face);
        const rim = Array.from({ length: 5 }, (_, i) => {
            const c = ((i + 0.4 * rand()) / 5) * 2 * Math.PI;
            return centre.clone().addScaledVector(side, 0.085 * Math.cos(c)).addScaledVector(up, 0.085 * Math.sin(c));
        });
        if (hash(centre, seed) >= BLOOM[season]) continue;
        for (let i = 0; i < 5; i++) cards.add([centre, rim[i], rim[(i + 1) % 5]], [lit, petal, i % 2 ? shade : petal], () => light(centre));
    }
    const shell = soft(lathe(OUTLINE.map(([z, r]) => [0.88 * r, z]), 10, 3));
    const fill = paint(relight(shell, light), (p) => inside.clone().lerp(mid, 0.3 * smooth(0.3, 0.9, p.z)));
    return merge([fill, cards.build()]);
}

export default {
    name: 'Oleander',
    latin: 'Nerium oleander',
    build: (season, seed) => oleander(season, seed, random(seed)),
    ground: { on: ['shrub', 'bare'], height: [2, 4], share: 0.01, own: true, clumped: true },
} satisfies Plant;
