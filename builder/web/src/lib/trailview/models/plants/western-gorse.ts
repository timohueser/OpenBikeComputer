import type { Plant } from '../plant-library';
import { core, domed, GOLDEN, hash, hex, ramp, random, sheet, smooth, tufts, v3, type Geometry, type Ramp } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** Dark green spines, browner in winter. */
const SPINES: Record<Season, Ramp> = {
    winter: ['#28341e', '#40502c', '#62703e'],
    spring: ['#243a1e', '#3a582a', '#5e7c3e'],
    summer: ['#24381e', '#38562a', '#5a783c'],
    autumn: ['#26381e', '#3a542a', '#5c763c'],
};

/** The share of spray tips in flower: it blooms from late summer into autumn, with heather. */
const BLOOM: Record<Season, number> = { winter: 0.06, spring: 0.04, summer: 0.45, autumn: 0.5 };
const FLOWER = hex('#f2c41c');

/**
 * Western gorse: a low, dense cushion about three times as wide as tall, six clumps of stiff spiny
 * sprays round dark cores. Each spray is one narrow triangle from inside its clump to a ragged
 * point on the clump's dome; in bloom the upper tips are yellow. 216 triangles.
 */
function westernGorse(season: Season, seed: number, rand: () => number): Geometry {
    const [deep, mid, tip] = ramp(SPINES[season]);
    const shade = deep.clone().multiplyScalar(0.7);
    const cards = sheet();
    for (const clump of tufts([0, 0, 0.65, 1], 5, 0.7, [0.6, 0.5, 0.6], [0.15, 0.12, 0.2], rand)) {
        const [x, y, r, h] = clump;
        const light = domed(v3(x, y, 0), v3(r, r, h), 0.3);
        core(cards, clump, shade, deep, 0.75, 0.75);
        for (let j = 0; j < 30; j++) {
            const a = j * GOLDEN + rand() * 0.6;
            const polar = 1.5 * Math.sqrt((j + 0.5) / 30);
            const k = 0.9 + 0.15 * rand();
            const top = v3(x + r * k * Math.sin(polar) * Math.cos(a), y + r * k * Math.sin(polar) * Math.sin(a), h * k * Math.cos(polar) ** 0.5);
            // Sprays bristle out from the dome more steeply than heather's.
            const lean = 0.6 * polar;
            const foot = top.clone().addScaledVector(v3(Math.sin(lean) * Math.cos(a), Math.sin(lean) * Math.sin(a), Math.cos(lean)), -h * (0.4 + 0.15 * rand()));
            foot.z = Math.max(foot.z, 0);
            const turn = a + Math.PI / 2 + (rand() - 0.5) * 0.8;
            const side = v3(Math.cos(turn), Math.sin(turn), 0).multiplyScalar(0.045 * h * (0.8 + 0.4 * rand()));
            const low = 1 - smooth(0.2, 0.9, top.z / h);
            const spray = mid.clone().lerp(tip, 0.4 * rand()).lerp(deep, 0.4 * low);
            const corners = [top.clone().add(side), top.clone().sub(side)];
            for (const c of corners) c.z -= 0.06 * h * rand();
            // Flowers sit on the upper tips, picked by hash so a bush keeps its shape in every season.
            const tips = corners.map((c) => (hash(c, seed) < BLOOM[season] * (1 - low) ? spray.clone().lerp(FLOWER, 0.8) : spray));
            cards.add([foot, ...corners], [shade, ...tips], light);
        }
    }
    return cards.build();
}

export default {
    name: 'Western gorse',
    latin: 'Ulex gallii',
    build: (season, seed) => westernGorse(season, seed, random(seed)),
    ground: { on: ['shrub', 'moss'], height: [0.2, 0.6], share: 0.3, own: true },
} satisfies Plant;
