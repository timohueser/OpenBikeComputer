import type { Plant } from '../plant-library';
import { domed, GOLDEN, hex, kite, ramp, random, sheet, spindle, v3, type Geometry, type Ramp, type V } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** Live bristle leaves: grey-green, with a blue cast. */
const MAT_GREEN: Ramp = ['#4a6040', '#80a06c', '#b4c89c'];

/** Dead leaves and old culms, bleached nearly white by winter. */
const MAT_STRAW: Record<Season, Ramp> = {
    winter: ['#9a9074', '#cdc4a6', '#ece6d2'],
    spring: ['#948662', '#c6b994', '#e4dcc0'],
    summer: ['#968460', '#c4b48c', '#e2d6b2'],
    autumn: ['#9c8658', '#ccb886', '#eadcb2'],
};

/** Share of the leaves still green, counted from the middle of the tuft out. */
const MAT_LIVE: Record<Season, number> = { winter: 0, spring: 0.5, summer: 0.8, autumn: 0.3 };

/** The slim spike, foot and top: dark violet in flower, straw once it has seeded. */
const SPIKE: Record<Season, [string, string]> = {
    winter: ['#a89c7c', '#cfc6a8'],
    spring: ['#9c9070', '#c4b896'],
    summer: ['#3e3448', '#5e5068'],
    autumn: ['#94845c', '#b8a678'],
};

/**
 * A tuft of mat-grass, round as a hedgehog: 28 stiff bristle leaves that splay straight out of a tight base, their
 * tips on one dome, a skirt of 13 bleached dead leaves flat round its foot, and three culms that stand above it,
 * each with a slim spike bent to one side. Live leaves grow from the middle out as the season has them. 103
 * triangles, 89 in spring, which has one culm.
 */
function matGrass(season: Season, rand: () => number): Geometry {
    const [greenDeep, greenMid, greenTip] = ramp(MAT_GREEN);
    const [deep, mid, tip] = ramp(MAT_STRAW[season]);
    const light = domed(v3(0, 0, 0.1), v3(0.5, 0.5, 0.7), 0.55);
    const cards = sheet();
    const reach = (a: number, angle: number, d: number) => v3(d * Math.sin(angle) * Math.cos(a), d * Math.sin(angle) * Math.sin(a), d * Math.cos(angle));
    const leaf = (foot: V, end: V, a: number, green: boolean) => {
        const r = rand();
        // Every leaf rises out of the pale dead sheaths that wrap the base.
        const body = green ? greenDeep.clone().lerp(greenMid, 0.5 + 0.5 * r).lerp(greenTip, 0.3 * rand()) : mid.clone().lerp(tip, 0.2 + 0.7 * r);
        const root = deep.clone().lerp(mid, 0.6).lerp(body, 0.35);
        kite(cards, foot, foot.clone().lerp(end, 0.4), end, reach(a + Math.PI / 2, Math.PI / 2, 0.04 + 0.012 * rand()), [root, body, body.clone().lerp(tip, 0.3)], light);
    };
    // Leaves leave the base at angles from upright to splayed, the upright ones a little longer.
    const leaves = 28;
    for (let j = 0; j < leaves; j++) {
        const u = (j + 0.5) / leaves, a = j * GOLDEN + rand() * 0.3;
        const foot = reach(a, Math.PI / 2, 0.06 * Math.sqrt(u));
        leaf(foot, foot.clone().add(reach(a, 0.1 + 0.9 * Math.sqrt(u), (0.68 - 0.1 * u) * (0.94 + 0.12 * rand()))), a, u < MAT_LIVE[season]);
    }
    // The skirt: dead leaves bent out over the ground round the base.
    for (let j = 0; j < 13; j++) {
        const a = ((j + rand() * 0.5) / 13) * 2 * Math.PI;
        leaf(reach(a, Math.PI / 2, 0.05).setZ(0.05), reach(a, Math.PI / 2, 0.42 + 0.08 * (j % 2) + 0.06 * rand()).setZ(0.04 + 0.05 * rand()), a, false);
    }
    const spike = SPIKE[season].map(hex), stem = season === 'summer' ? greenMid.clone().lerp(greenTip, 0.3) : mid;
    const culms = season === 'spring' ? 1 : 3;
    for (let k = 0; k < culms; k++) {
        const a = (k / 3) * 2 * Math.PI + rand() * 1.2, lean = 0.08 + 0.2 * rand(), height = 1 - 0.1 * k;
        const base = reach(a, lean, 0.72 * height), top = base.clone().add(reach(a, lean + 0.35, 0.28 * height));
        const side = reach(a + Math.PI / 2, Math.PI / 2, 0.014);
        cards.add([side, side.clone().negate(), base], [deep, deep, stem], light);
        spindle(cards, base, top, 0.035, 3, spike);
    }
    return cards.build();
}

export default {
    name: 'Mat-grass',
    latin: 'Nardus stricta',
    build: (season, seed) => matGrass(season, random(seed)),
    ground: { on: ['grass', 'moss'], height: [0.15, 0.3], share: 0.3, own: true },
} satisfies Plant;
