import type { Plant } from '../plant-library';
import { blade, circle, GOLDEN, hex, merge, octahedron, paint, place, random, ribbon, smooth, soft, turnFromTo, UP, v3, type Geometry, type Vec } from '../plant-shape';
import type { Season } from '../../rules/regions';

/**
 * Per season: the stem's colour, then the share of heads in flower and of heads gone to seed; the
 * rest are nodding buds. The quarters are long for a bloom of late May to July, so spring has buds
 * and the first flowers, summer the full bloom with seed heads, and autumn and winter dead stalks.
 */
const SEASON: Record<Season, [stem: string, flowers: number, capsules: number]> = {
    winter: ['#8e8670', 0, 1],
    spring: ['#5e8a42', 0.45, 0],
    summer: ['#6a8a44', 0.85, 0.15],
    autumn: ['#9a8a62', 0, 1],
};

const [LEAF, PETAL, LIT, BLOTCH, BUD, CAPSULE] = ['#5e7e48', '#e8140a', '#ff4420', '#1c1412', '#6e9048', '#8a9a6a'].map(hex);

/** A flower: a shallow cup of eight triangles round a black eye. */
function cup(r: number): Geometry {
    const geo = circle(r, 8);
    for (let i = 1; i < geo.position.length / 3; i++) geo.position[3 * i + 2] = 0.45 * r;
    return paint(soft(geo), (p) => BLOTCH.clone().lerp(PETAL, smooth(0.05 * r, 0.35 * r, Math.hypot(p.x, p.y))).lerp(LIT, smooth(0.6 * r, r, Math.hypot(p.x, p.y))));
}

/**
 * Corn poppy: a full tuft of seven slender stems from one foot, of a little different heights, each
 * ending in a scarlet cup turned partly to the side, a nodding bud or a seed capsule, as the season
 * has them; a few lobed leaves at the foot. 74 triangles in full flower.
 */
function poppy(season: Season, rand: () => number): Geometry {
    const [stemColour, flowers, capsules] = SEASON[season];
    const stem = hex(stemColour);
    const dead = flowers === 0;
    const parts: Geometry[] = [];
    for (let k = 0; k < 4; k++) {
        const a = (k / 4) * 2 * Math.PI + rand();
        parts.push(paint(blade([0, 0, 0], a, 0.28, 1.2, 0.3, (t) => 0.12 * Math.sin(Math.PI * Math.min(1, t + 0.1)), 1), dead ? stem : LEAF));
    }
    for (let k = 0; k < 7; k++) {
        const a = k * GOLDEN + rand() * 0.5;
        const out = 0.06 + 0.3 * Math.sqrt((k + 0.5) / 7);
        const foot: Vec = [0.15 * out * Math.cos(a), 0.15 * out * Math.sin(a), 0];
        const h = 0.7 + 0.3 * rand();
        const top: Vec = [out * Math.cos(a), out * Math.sin(a), h];
        parts.push(paint(ribbon([foot, top], [-Math.sin(a), Math.cos(a), 0], () => 0.022, 1), stem));
        // The same draws in every season, so a plant keeps its shape the year round. A cup leans out
        // from the tuft, so it shows from the side as well as from above.
        const [roll, lean] = [rand(), 0.3 + 0.4 * rand()];
        if (roll < flowers) parts.push(place(cup(0.16), { at: top, turn: turnFromTo(UP, UP.clone().applyAxisAngle(v3(-Math.sin(a), Math.cos(a), 0), lean)) }));
        else if (roll < flowers + capsules) parts.push(place(paint(soft(octahedron(0.035)), dead ? stem : CAPSULE), { at: top, scale: [1, 1, 1.4] }));
        // A bud hangs from the crook at the stem's top.
        else parts.push(place(paint(soft(octahedron(0.03)), BUD), { at: [top[0] + 0.04 * Math.cos(a), top[1] + 0.04 * Math.sin(a), top[2] - 0.05], scale: [1, 1, 1.5] }));
    }
    return merge(parts);
}

export default {
    name: 'Corn poppy',
    latin: 'Papaver rhoeas',
    build: (season, seed) => poppy(season, random(seed)),
    ground: { on: ['grass', 'crop'], height: [0.3, 0.6], share: 0.05, own: true, clumped: true },
} satisfies Plant;
