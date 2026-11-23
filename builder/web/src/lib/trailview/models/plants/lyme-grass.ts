import type { Plant } from '../plant-library';
import { domed, GOLDEN, hex, kite, ramp, random, sheet, spindle, v3, type Geometry, type Ramp } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** Lyme grass blades: blue-grey with a waxy bloom, straw at the tips by autumn. */
const LYME: Record<Season, Ramp> = {
    winter: ['#6a6e64', '#9a9c8c', '#c4bea4'],
    spring: ['#4e6466', '#80989a', '#b4c6c2'],
    summer: ['#4c6264', '#7c9496', '#b0c2be'],
    autumn: ['#5e6a62', '#909c90', '#c4c0a4'],
};

/** The thick ears, foot and top: green-grey in summer, straw otherwise. */
const EARS: Record<Season, [string, string]> = {
    winter: ['#9a8e6c', '#c8bc98'],
    spring: ['#78908a', '#a4b6ac'],
    summer: ['#8a9c84', '#b8c4a4'],
    autumn: ['#a89668', '#d8c89a'],
};

/**
 * Lyme grass: a stiff sheaf of 21 broad blades that stand up and only bow out at the rim, and three thick ears
 * on straight stalks above it.
 */
function lymeGrass(season: Season, rand: () => number): Geometry {
    const [deep, mid, tip] = ramp(LYME[season]);
    const light = domed(v3(), v3(0.6, 0.6, 1), 0.35);
    const cards = sheet();
    const reach = (a: number, angle: number, d: number) => v3(d * Math.sin(angle) * Math.cos(a), d * Math.sin(angle) * Math.sin(a), d * Math.cos(angle));
    const blades = 21;
    for (let k = 0; k < blades; k++) {
        const u = (k + 0.5) / blades, a = k * GOLDEN + rand() * 0.4, length = 0.64 + 0.18 * rand();
        const tilt = 0.06 + 0.6 * Math.sqrt(u), bow = 0.1 + 0.45 * u;
        const foot = reach(a, Math.PI / 2, 0.02 + 0.07 * Math.sqrt(u));
        const waist = foot.clone().add(reach(a, tilt, 0.5 * length)), end = waist.clone().add(reach(a, tilt + bow, 0.5 * length));
        kite(cards, foot, waist, end, reach(a + Math.PI / 2, Math.PI / 2, 0.055 + 0.015 * rand()), [deep, mid.clone().lerp(tip, 0.3 * rand()), tip], light);
    }
    const ear = EARS[season].map(hex);
    for (let k = 0; k < 3; k++) {
        const a = (k / 3) * 2 * Math.PI + rand() * 1.5, lean = 0.05 + 0.14 * rand(), height = 0.9 + 0.1 * ((k * 2) % 3) / 2;
        const base = reach(a, lean, 0.7 * height), top = reach(a, lean, height);
        const side = reach(a + Math.PI / 2, Math.PI / 2, 0.014);
        cards.add([side, side.clone().negate(), base], [deep, deep, mid], light);
        spindle(cards, base, top, 0.045, 3, ear);
    }
    return cards.build();
}

export default {
    name: 'Lyme grass',
    latin: 'Leymus arenarius',
    build: (season, seed) => lymeGrass(season, random(seed)),
    ground: { on: ['bare'], height: [0.5, 1], share: 0.05, own: true, clumped: true },
} satisfies Plant;
