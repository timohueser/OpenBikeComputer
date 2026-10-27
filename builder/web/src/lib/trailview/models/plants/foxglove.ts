import type { Plant } from '../plant-library';
import { blade, byHeight, domed, hex, merge, paint, ramp, random, ribbon, sheet, v3, type Geometry, type Ramp, type Vec } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** The rosette's broad, soft leaves: dark green, a little duller in winter. */
const LEAVES: Record<Season, Ramp> = {
    winter: ['#2e4226', '#4a6038', '#6e8052'],
    spring: ['#2e4a26', '#4e703a', '#7a9a58'],
    summer: ['#2e4826', '#4c6c38', '#769456'],
    autumn: ['#3a4628', '#5a663a', '#808650'],
};

/**
 * Per season: the spike's height, its stalk colour, and the share of its places that hold an open
 * bell, from the foot up; the rest hold buds, or seed capsules once it has flowered. Winter has
 * the leaf rosette alone.
 */
const SPIKE: Record<Season, [height: number, stalk: string, bells: number, seeds: boolean]> = {
    winter: [0, '#4a6038', 0, false],
    spring: [0.75, '#5a7a44', 0.3, false],
    summer: [1, '#5a7a44', 0.8, false],
    autumn: [0.95, '#7a6448', 0, true],
};

const [BELL, RIM, BUD, CAPSULE] = ['#a03c88', '#e07cc0', '#7a9a5a', '#6a5038'].map(hex);

/**
 * Foxglove: a rosette of broad leaves and one tall spike hung on one side with nodding purple-pink
 * bells, each two cards that open downwards from the stalk. It builds itself 1 tall in flower, so
 * the winter rosette stays low. 61 triangles in flower.
 */
function foxglove(season: Season, rand: () => number): Geometry {
    const [height, stalk, bells, seeds] = SPIKE[season];
    const parts: Geometry[] = [];
    const leaf = byHeight(ramp(LEAVES[season]), 0.15);
    for (let k = 0; k < 7; k++) {
        const a = (k / 7) * 2 * Math.PI + rand() * 0.5;
        parts.push(paint(blade([0, 0, 0], a, 0.28 + 0.06 * rand(), 1.2, 0.35, (t) => 0.13 * Math.sin(Math.PI * Math.min(1, t + 0.1)), 2), leaf));
    }
    const face = rand() * 2 * Math.PI;
    const lean = 0.04 * rand();
    if (!height) return merge(parts);
    const top: Vec = [lean * Math.cos(face), lean * Math.sin(face), height];
    parts.push(paint(ribbon([[0, 0, 0], [0.5 * top[0], 0.5 * top[1], 0.5 * height], top], [-Math.sin(face), Math.cos(face), 0], (t) => 0.025 * (1 - 0.6 * t), 2), hex(stalk)));
    const cards = sheet();
    const light = domed(v3(0, 0, 0.6 * height), v3(0.3, 0.3, 0.6), 0.3);
    for (let i = 0; i < 20; i++) {
        const t = 0.35 + (0.6 * i) / 20;
        // The bells keep to one side of the spike, a little spread round it.
        const a = face + (rand() - 0.5) * 1.6;
        const at = v3(t * top[0], t * top[1], t * height);
        const out = v3(Math.cos(a), Math.sin(a), 0);
        const side = v3(-out.y, out.x, 0);
        const size = 0.1 * (1.15 - 0.6 * (i / 20));
        // Bells open from the foot of the spike up.
        const open = i < Math.round(20 * bells);
        if (open) {
            const mouth = at.clone().addScaledVector(out, size).setZ(at.z - 0.8 * size);
            cards.add([at, mouth.clone().addScaledVector(side, 0.5 * size), mouth.clone().addScaledVector(side, -0.5 * size)], [BELL, RIM, RIM], light);
            cards.add([at, mouth.clone().add(v3(0, 0, 0.35 * size)), mouth.clone().add(v3(0, 0, -0.35 * size))], [BELL, RIM, BELL], light);
        } else {
            const end = at.clone().addScaledVector(out, 0.4 * size).setZ(at.z + 0.2 * size);
            cards.add([at.clone().addScaledVector(side, 0.25 * size), at.clone().addScaledVector(side, -0.25 * size), end], [seeds ? CAPSULE : BUD, seeds ? CAPSULE : BUD, seeds ? CAPSULE : BUD], light);
        }
    }
    parts.push(cards.build());
    return merge(parts);
}

export default {
    name: 'Foxglove',
    latin: 'Digitalis purpurea',
    build: (season, seed) => foxglove(season, random(seed)),
    ground: { on: ['grass', 'shrub'], height: [0.8, 1.5], share: 0.03, own: true, clumped: true },
    sized: true,
} satisfies Plant;
