import type { Plant } from '../plant-library';
import { core, domed, GOLDEN, hex, ramp, random, sheet, smooth, tufts, v3, type Geometry, type Ramp } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** Dwarf birch leaves: dark glossy green, rust-red to orange in autumn. */
const DWARF_BIRCH: Record<Season, Ramp> = {
    // Bare red-brown twigs: the leaves take the twigs' colour.
    winter: ['#2e1e1c', '#4a302a', '#6a4236'],
    spring: ['#34602a', '#52863a', '#84b058'],
    summer: ['#24401e', '#3a5c2c', '#5e8040'],
    autumn: ['#5c1a14', '#9a381e', '#c8602c'],
};

const STEM = hex('#4a2c26');

/**
 * Dwarf birch: a low, dense mound about three times as wide as tall, six clumps of leafy twigs
 * round dark cores. Each twig is one triangle from a narrow foot inside its clump to a wide, ragged
 * top on the clump's dome, where its small round leaves are. 216 triangles.
 */
function dwarfBirch(season: Season, rand: () => number): Geometry {
    const [deep, mid, tip] = ramp(DWARF_BIRCH[season]);
    const shade = deep.clone().lerp(STEM, 0.5).multiplyScalar(0.7);
    const cards = sheet();
    for (const clump of tufts([0, 0, 0.7, 1], 5, 0.7, [0.85, 0.55, 0.6], [0.15, 0.12, 0.25], rand)) {
        const [x, y, r, h] = clump;
        const light = domed(v3(x, y, 0), v3(r, r, h), 0.35);
        core(cards, clump, shade, deep, 0.7, 0.7);
        for (let j = 0; j < 30; j++) {
            const a = j * GOLDEN + rand() * 0.6;
            const polar = 1.5 * Math.sqrt((j + 0.5) / 30);
            const k = 0.9 + 0.15 * rand();
            const top = v3(x + r * k * Math.sin(polar) * Math.cos(a), y + r * k * Math.sin(polar) * Math.sin(a), h * k * Math.cos(polar) ** 0.5);
            // Twigs fan out more than they lean with the dome, so the mound reads as a thicket.
            const lean = 0.5 * polar;
            const foot = top.clone().addScaledVector(v3(Math.sin(lean) * Math.cos(a), Math.sin(lean) * Math.sin(a), Math.cos(lean)), -h * (0.4 + 0.15 * rand()));
            foot.z = Math.max(foot.z, 0);
            const turn = a + Math.PI / 2 + (rand() - 0.5) * 0.8;
            const side = v3(Math.cos(turn), Math.sin(turn), 0).multiplyScalar(0.12 * (0.8 + 0.4 * rand()));
            const low = 1 - smooth(0.2, 0.9, top.z / h);
            const leaf = mid.clone().lerp(tip, 0.6 * rand()).lerp(deep, 0.45 * low);
            const corners = [top.clone().add(side), top.clone().sub(side)];
            for (const c of corners) c.z -= 0.06 * rand();
            cards.add([foot, ...corners], [shade, leaf, leaf.clone().lerp(mid, 0.3 * rand())], light);
        }
    }
    return cards.build();
}

export default {
    name: 'Dwarf birch',
    latin: 'Betula nana',
    build: (season, seed) => dwarfBirch(season, random(seed)),
    ground: { on: ['shrub', 'moss', 'grass'], height: [0.2, 0.5], share: 0.3, own: true },
} satisfies Plant;
