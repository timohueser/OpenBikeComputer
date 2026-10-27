import type { Plant } from '../plant-library';
import { blade, circle, GOLDEN, hex, merge, octahedron, paint, place, random, ribbon, smooth, soft, turnFromTo, UP, v3, type Geometry, type Vec } from '../plant-shape';
import type { Season } from '../../rules/regions';

/**
 * Per season: the stem's colour, then the share of heads in flower; the rest are grey-green buds,
 * or dry heads on a dead plant. The quarters are long for a bloom of late May to July, so spring
 * has buds and the first flowers, summer the full bloom, and autumn and winter dead stalks.
 */
const SEASON: Record<Season, [stem: string, flowers: number]> = {
    winter: ['#8e8670', 0],
    spring: ['#7a9a6e', 0.35],
    summer: ['#7e9a70', 0.8],
    autumn: ['#9a8a62', 0],
};

const [RAY, LIT, DISC, BUD, DRY] = ['#1f50e0', '#4a84ff', '#3a2c8a', '#7a9070', '#7a6a4e'].map(hex);

/** A flower facing up: five ragged blue rays round a dark violet disc, ten triangles. */
function head(r: number): Geometry {
    const geo = circle(r, 10);
    for (let i = 1; i < geo.position.length / 3; i++) geo.position.splice(3 * i, 3, geo.position[3 * i] * (i % 2 ? 1 : 0.6), geo.position[3 * i + 1] * (i % 2 ? 1 : 0.6), 0.3 * r);
    return paint(soft(geo), (p) => DISC.clone().lerp(RAY, smooth(0.1 * r, 0.5 * r, Math.hypot(p.x, p.y))).lerp(LIT, smooth(0.7 * r, r, Math.hypot(p.x, p.y))));
}

/**
 * Cornflower: a tuft of four thin, wiry stems from one foot, of a little different heights, each
 * with a blue flower turned partly to the side or a scaly grey-green bud, as the season has them; two
 * fork once below the top into a smaller bud; a few narrow leaves at the foot. 72 triangles in flower.
 */
function cornflower(season: Season, rand: () => number): Geometry {
    const [stemColour, flowers] = SEASON[season];
    const stem = hex(stemColour);
    const dead = flowers === 0;
    const parts: Geometry[] = [];
    for (let k = 0; k < 4; k++) {
        const a = (k / 4) * 2 * Math.PI + rand();
        parts.push(paint(blade([0, 0, 0], a, 0.3, 0.9, 0.25, (t) => 0.05 * Math.sin(Math.PI * Math.min(1, t + 0.1)), 1), stem));
    }
    const bud = (at: Vec, size: number) => parts.push(place(paint(soft(octahedron(size)), dead ? DRY : BUD), { at, scale: [1, 1, 1.3] }));
    for (let k = 0; k < 4; k++) {
        const a = k * GOLDEN + rand() * 0.5;
        const out = 0.06 + 0.25 * Math.sqrt((k + 0.5) / 4);
        const foot: Vec = [0.15 * out * Math.cos(a), 0.15 * out * Math.sin(a), 0];
        const h = 0.7 + 0.3 * rand();
        const top: Vec = [out * Math.cos(a), out * Math.sin(a), h];
        const across: Vec = [-Math.sin(a), Math.cos(a), 0];
        parts.push(paint(ribbon([foot, top], across, () => 0.018, 1), stem));
        // The same draws in every season, so a plant keeps its shape the year round. A flower leans
        // out from the tuft, so it shows from the side as well as from above.
        const [roll, lean, side] = [rand(), 0.3 + 0.4 * rand(), a + (rand() < 0.5 ? 1 : -1) * 0.8];
        if (roll < flowers) parts.push(place(head(0.12), { at: top, turn: turnFromTo(UP, UP.clone().applyAxisAngle(v3(...across), lean)) }));
        else bud(top, 0.03);
        if (k % 2) continue;
        const fork: Vec = [0.7 * top[0], 0.7 * top[1], 0.7 * h];
        const branch: Vec = [fork[0] + 0.12 * Math.cos(side), fork[1] + 0.12 * Math.sin(side), fork[2] + 0.15];
        parts.push(paint(ribbon([fork, branch], across, () => 0.014, 1), stem));
        bud(branch, 0.022);
    }
    return merge(parts);
}

export default {
    name: 'Cornflower',
    latin: 'Centaurea cyanus',
    build: (season, seed) => cornflower(season, random(seed)),
    ground: { on: ['grass', 'crop'], height: [0.3, 0.7], share: 0.04, own: true, clumped: true },
} satisfies Plant;
