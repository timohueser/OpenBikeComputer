import type { Plant } from '../plant-library';
import { domed, GOLDEN, hex, kite, ramp, random, sheet, spindle, v3, type Geometry, type Ramp } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** Esparto blades: grey-green at the root, pale straw at the tip. */
const ESPARTO: Record<Season, Ramp> = {
    winter: ['#66654a', '#9a9672', '#c4b98e'],
    spring: ['#5f6a40', '#98a06a', '#d2c88e'],
    summer: ['#6b6a45', '#a39d6a', '#d8c98e'],
    autumn: ['#6b6a45', '#a39d6a', '#d4c58e'],
};

/** The feathery plumes, foot and top: silvery green as they open, gold in summer, bleached after. */
const PLUMES: Record<Season, [string, string]> = {
    winter: ['#a89c7c', '#d2c8aa'],
    spring: ['#a8ae7c', '#dcdcae'],
    summer: ['#c4a862', '#ecd89a'],
    autumn: ['#b4a274', '#dccfa6'],
};

/**
 * Esparto: a round tussock of 25 long wiry blades that arch out of the crown, grey-green below and straw at the
 * tips, and four long plumes that nod on stalks above it. 78 triangles.
 */
function esparto(season: Season, rand: () => number): Geometry {
    const [deep, mid, tip] = ramp(ESPARTO[season]);
    const light = domed(v3(), v3(1, 1, 1), 0.35);
    const cards = sheet();
    const reach = (a: number, angle: number, d: number) => v3(d * Math.sin(angle) * Math.cos(a), d * Math.sin(angle) * Math.sin(a), d * Math.cos(angle));
    const blades = 25;
    for (let k = 0; k < blades; k++) {
        const u = (k + 0.5) / blades, a = k * GOLDEN + rand() * 0.4, length = 0.85 + 0.3 * rand();
        // Inner blades stand, outer ones bow out; no tip reaches the ground.
        const tilt = 0.08 + 0.85 * Math.sqrt(u), bow = Math.min(0.35 + 0.6 * u, 1.5 - tilt);
        const foot = reach(a, Math.PI / 2, 0.03 + 0.07 * Math.sqrt(u));
        const waist = foot.clone().add(reach(a, tilt, 0.5 * length)), end = waist.clone().add(reach(a, tilt + bow, 0.5 * length));
        const body = mid.clone().lerp(tip, 0.3 * rand());
        kite(cards, foot, waist, end, reach(a + Math.PI / 2, Math.PI / 2, 0.034 + 0.012 * rand()), [deep, body, tip.clone().lerp(mid, 0.4 * rand())], light);
    }
    const plume = PLUMES[season].map(hex);
    for (let k = 0; k < 4; k++) {
        const a = (k / 4) * 2 * Math.PI + rand() * 1.2, lean = 0.12 + 0.2 * rand(), height = 0.95 + 0.2 * rand();
        const base = reach(a, lean, 0.72 * height), top = base.clone().add(reach(a, lean + 0.5 + 0.4 * rand(), 0.34 * height));
        const side = reach(a + Math.PI / 2, Math.PI / 2, 0.012);
        cards.add([side, side.clone().negate(), base], [deep, deep, mid], light);
        spindle(cards, base, top, 0.035, 3, plume);
    }
    return cards.build();
}

export default {
    name: 'Esparto',
    latin: 'Macrochloa tenacissima',
    build: (season, seed) => esparto(season, random(seed)),
    ground: { on: ['grass', 'bare', 'shrub'], height: [0.4, 0.8], share: 0.4, own: true },
} satisfies Plant;
