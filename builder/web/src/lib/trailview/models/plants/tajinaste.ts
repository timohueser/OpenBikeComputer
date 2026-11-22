import type { Plant } from '../plant-library';
import { GOLDEN, hash, hex, leaf, merge, paint, ramp, random, revolve, sheet, smooth, soft, UP, v3, type Geometry, type Ramp } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** Silver rosette leaves of the tajinaste. */
const SILVER: Ramp = ['#5e6a5a', '#9aa496', '#cdd2c4'];

/** The spike: coral red in flower (buds, flowers, bract tips), spent by late summer, dry in winter. */
const SPIKE: Record<Season, Ramp> = {
    winter: ['#5a4434', '#7e6048', '#a08262'],
    spring: ['#8e3036', '#d8505a', '#f08a84'],
    summer: ['#6a3a2c', '#9a5a40', '#c08262'],
    autumn: ['#5e4636', '#84644a', '#a68866'],
};

/** The spike's outline, (radius, height): thickest low down, a long taper to a blunt tip. */
const OUTLINE: [number, number][] = [[0.06, 0], [0.095, 0.2], [0.1, 0.38], [0.08, 0.6], [0.05, 0.82], [0.022, 0.95], [0, 1]];
const girth = (z: number) => {
    const i = Math.max(1, OUTLINE.findIndex(([, at]) => at >= z));
    const [[r0, z0], [r1, z1]] = [OUTLINE[i - 1], OUTLINE[i]];
    return r0 + ((r1 - r0) * (z - z0)) / (z1 - z0);
};

/** The whorls of flowers up the spike, the flowers of a whorl, and the leaves of the rosette. */
const WHORLS = 15, FLOWERS = 6, LEAVES = 30;

/**
 * Teide bugloss in flower: one tall spike out of a mound of narrow silver leaves. The spike is a round tapering
 * column, and whorls of flower sprays stand out of it and up, each whorl turned half a step against the one below,
 * so its outline is bristly from every side; the lowest whorls are still silver leaves.
 */
function tajinaste(season: Season, seed: number, rand: () => number): Geometry {
    const [deep, silver, pale] = ramp(SILVER);
    const [dark, body, lit] = ramp(SPIKE[season]);
    const bract = hex('#b4bcac');
    const leafy = (z: number) => 1 - smooth(0.1, 0.26, z);
    const cards = sheet();
    for (let w = 0; w < WHORLS; w++) for (let i = 0; i < FLOWERS; i++) {
        const z = 0.1 + 0.82 * (w / (WHORLS - 1)) + 0.02 * rand(), a = ((i + 0.5 * (w % 2) + 0.3 * rand()) / FLOWERS) * 2 * Math.PI;
        const out = v3(Math.cos(a), Math.sin(a), 0), at = out.clone().multiplyScalar(0.85 * girth(z)).setZ(z);
        const colour = body.clone().lerp(lit, 0.2 + 0.6 * smooth(0.4, 1, z));
        if (hash(at, seed + 3) < 0.3) colour.lerp(bract, 0.5);
        colour.lerp(silver, leafy(z));
        const length = (0.04 + 0.75 * girth(z)) * (0.85 + 0.3 * rand()), lean = out.clone().lerp(UP, 0.35).normalize();
        leaf(cards, at, out.clone().multiplyScalar(0.8).setZ(0.6), length, 0.6 * length, 0.45, [dark.clone().lerp(deep, leafy(z)), colour, colour.clone().lerp(lit.clone().lerp(pale, leafy(z)), 0.5)], () => lean);
    }
    for (let k = 0; k < LEAVES; k++) {
        const a = k * GOLDEN + 0.4 * rand(), tilt = 0.45 + 0.95 * Math.sqrt((k + 0.5) / LEAVES) * (0.85 + 0.15 * rand());
        const out = v3(Math.cos(a), Math.sin(a), 0), lean = out.clone().lerp(UP, 0.5).normalize();
        const tone = silver.clone().lerp(pale, 0.6 * rand());
        leaf(cards, out.clone().multiplyScalar(0.04), out.clone().multiplyScalar(Math.sin(tilt)).setZ(Math.cos(tilt)), 0.36 * (0.8 + 0.35 * rand()), 0.085, 0.45, [deep, tone, tone.clone().lerp(pale, 0.6)], () => lean);
    }
    const spike = paint(soft(revolve(OUTLINE, 6)), (p) => dark.clone().lerp(body, 0.4).lerp(silver, leafy(p.z)));
    return merge([spike, cards.build()]);
}

export default {
    name: 'Tajinaste',
    latin: 'Echium wildpretii',
    build: (season, seed) => tajinaste(season, seed, random(seed)),
    ground: { on: ['shrub', 'bare'], height: [1.5, 2.5], share: 0.02, own: true },
} satisfies Plant;
