import type { Plant } from '../plant-library';
import { domed, GOLDEN, hex, kite, ramp, random, sheet, spindle, v3, type Geometry, type Ramp } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** Purple moor grass and deer grass: the straw of dead blades, tawny to orange-brown in autumn. */
const MOOR_GRASS: Record<Season, Ramp> = {
    winter: ['#6a4e32', '#a8845a', '#d4b88a'],
    spring: ['#5e4a30', '#9a7e56', '#c8ac7e'],
    summer: ['#5a4e30', '#968458', '#d2bc88'],
    autumn: ['#6a3a1e', '#a8683a', '#c99a5a'],
};

const MOOR_GREEN: Ramp = ['#3a4a1f', '#7a8c3e', '#a2ac58'];

/** Share of a tussock's blades still green, counted from the crown out. */
const MOOR_LIVE: Record<Season, number> = { winter: 0, spring: 0.3, summer: 0.95, autumn: 0.12 };

/** The slim flower heads, foot and top: purple in flower, then the straw of the stalks that stand through winter. */
const HEADS: Record<Season, [string, string]> = {
    winter: ['#8a7252', '#b89c74'],
    spring: ['#8a7658', '#b4a07a'],
    summer: ['#4a3350', '#7a5a7c'],
    autumn: ['#5c3a34', '#8c5e4a'],
};

/**
 * A tussock of purple moor grass or deer grass, lit as one dome: a fountain of 31 blades, upright in the crown and
 * ever longer and more bowed toward the rim, where they arch over toward the ground, and five thin stalks that
 * stand above it with a slim flower head each. Live blades grow from the crown out as the season has them, dead
 * straw beyond.
 */
function moorGrass(season: Season, rand: () => number): Geometry {
    const [deep, mid, tip] = ramp(MOOR_GRASS[season]);
    const [greenDeep, greenMid] = ramp(MOOR_GREEN);
    // Live blades go buff at the tip only as they turn in autumn.
    const buff = season === 'autumn' ? 0.35 : 0.1;
    const light = domed(v3(), v3(0.8, 0.8, 0.8), 0.35);
    const cards = sheet();
    const reach = (a: number, angle: number, d: number) => v3(d * Math.sin(angle) * Math.cos(a), d * Math.sin(angle) * Math.sin(a), d * Math.cos(angle));
    const blades = 31;
    for (let j = 0; j < blades; j++) {
        const u = (j + 0.5) / blades, a = j * GOLDEN + rand() * 0.4;
        const tilt = 0.1 + 1.0 * Math.sqrt(u), bow = 0.4 + 0.65 * u, length = (0.86 + 0.3 * u) * (0.9 + 0.2 * rand());
        const foot = reach(a, Math.PI / 2, 0.08 * Math.sqrt(u));
        const waist = foot.clone().add(reach(a, tilt, 0.5 * length)), end = waist.clone().add(reach(a, tilt + bow, 0.5 * length));
        end.z = Math.max(end.z, 0.08 + 0.12 * rand());
        const [r1, r2] = [rand(), rand()], live = u < MOOR_LIVE[season];
        const body = live ? greenMid.clone().lerp(tip, 0.5 * buff + 0.15 * r1) : mid.clone().lerp(tip, 0.35 + 0.4 * r1);
        const ends = live ? greenMid.clone().lerp(tip, buff + 0.35 * buff * r2 + 0.15 * r2) : body.clone().lerp(tip, 0.3);
        const root = (live ? greenDeep : deep).clone().lerp(body, 0.3);
        kite(cards, foot, waist, end, reach(a + Math.PI / 2, Math.PI / 2, 0.045 + 0.015 * rand()), [root, body, ends], light);
    }
    const head = HEADS[season].map(hex), stalk = season === 'summer' ? greenMid.clone().lerp(tip, 0.3) : mid;
    for (let k = 0; k < 5; k++) {
        const a = k * GOLDEN * 2 + rand(), lean = 0.1 + 0.3 * rand(), height = 0.88 + 0.12 * ((k * 2) % 5) / 4;
        const foot = reach(a, Math.PI / 2, 0.03), base = foot.clone().add(reach(a, lean, 0.78 * height)), top = base.clone().add(reach(a, lean + 0.25, 0.22 * height));
        const side = reach(a + Math.PI / 2, Math.PI / 2, 0.012);
        cards.add([foot.clone().add(side), foot.clone().sub(side), base], [deep, deep, stalk], light);
        spindle(cards, base, top, 0.04, 3, head);
    }
    return cards.build();
}

export default {
    name: 'Moor grass',
    latin: 'Molinia caerulea, Trichophorum cespitosum',
    build: (season, seed) => moorGrass(season, random(seed)),
    ground: { on: ['grass', 'shrub', 'wetland', 'moss'], height: [0.3, 0.6], share: 0.35, own: true },
} satisfies Plant;
