import type { Plant } from '../plant-library';
import { core, domed, GOLDEN, hex, ramp, random, sheet, smooth, tufts, v3, type Geometry, type Ramp } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** Bog myrtle leaves: grey-green, dull olive-yellow in autumn; in winter the bare red-brown twigs. */
const MYRTLE: Record<Season, Ramp> = {
    winter: ['#3a2420', '#6a3a2c', '#8a5040'],
    spring: ['#40503a', '#68785a', '#9aa686'],
    summer: ['#3c4a36', '#64725a', '#98a488'],
    autumn: ['#585230', '#867c46', '#ae9e62'],
};

/** Orange-brown catkins on the twig tips before the leaves: their share of the tips, by season. */
const CATKINS: Record<Season, number> = { winter: 0.15, spring: 0.35, summer: 0, autumn: 0 };
const CATKIN = hex('#b06a3a');

/**
 * Bog myrtle: a dense, upright bush a little taller than wide, five clumps of twigs round dark
 * cores. Each twig is one triangle from a narrow foot to a ragged top on its clump's dome, wide
 * with leaves, thin when bare. 180 triangles.
 */
function bogMyrtle(season: Season, rand: () => number): Geometry {
    const [deep, mid, tip] = ramp(MYRTLE[season]);
    const bare = season === 'winter';
    const shade = deep.clone().multiplyScalar(0.7);
    const cards = sheet();
    for (const clump of tufts([0, 0, 0.45, 1], 4, 0.7, [0.38, 0.32, 0.65], [0.1, 0.08, 0.25], rand)) {
        const [x, y, r, h] = clump;
        const light = domed(v3(x, y, 0), v3(r, r, h), 0.3);
        core(cards, clump, shade, deep, 0.7, 0.8);
        for (let j = 0; j < 30; j++) {
            const a = j * GOLDEN + rand() * 0.6;
            const polar = 1.4 * Math.sqrt((j + 0.5) / 30);
            const k = 0.9 + 0.15 * rand();
            const top = v3(x + r * k * Math.sin(polar) * Math.cos(a), y + r * k * Math.sin(polar) * Math.sin(a), h * k * Math.cos(polar) ** 0.5);
            // Twigs stand nearly upright, as the bush's stiff shoots do.
            const lean = 0.25 * polar;
            const foot = top.clone().addScaledVector(v3(Math.sin(lean) * Math.cos(a), Math.sin(lean) * Math.sin(a), Math.cos(lean)), -h * (0.5 + 0.2 * rand()));
            foot.z = Math.max(foot.z, 0);
            const turn = a + Math.PI / 2 + (rand() - 0.5) * 0.8;
            const side = v3(Math.cos(turn), Math.sin(turn), 0).multiplyScalar((bare ? 0.03 : 0.1) * (0.8 + 0.4 * rand()));
            const low = 1 - smooth(0.2, 0.9, top.z / h);
            const twig = mid.clone().lerp(tip, 0.5 * rand()).lerp(deep, 0.4 * low);
            if (rand() < CATKINS[season] * (1 - low)) twig.lerp(CATKIN, 0.7);
            const corners = [top.clone().add(side), top.clone().sub(side)];
            for (const c of corners) c.z -= 0.06 * h * rand();
            cards.add([foot, ...corners], [shade, twig, twig.clone().lerp(mid, 0.3)], light);
        }
    }
    return cards.build();
}

export default {
    name: 'Bog myrtle',
    latin: 'Myrica gale',
    build: (season, seed) => bogMyrtle(season, random(seed)),
    ground: { on: ['wetland', 'moss'], height: [0.5, 1.2], share: 0.15, habitat: 'moor', own: true, clumped: true },
} satisfies Plant;
