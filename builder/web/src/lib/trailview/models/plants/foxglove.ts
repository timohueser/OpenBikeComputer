import type { Plant } from '../plant-library';
import { hex, leaf, ramp, random, sheet, spike, UP, v3, type Geometry, type Ramp, type V } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** The rosette's broad, soft leaves: dark green, a little duller in winter. */
const LEAVES: Record<Season, Ramp> = {
    winter: ['#2e4226', '#4a6038', '#6e8052'],
    spring: ['#2e4a26', '#4e703a', '#7a9a58'],
    summer: ['#2e4826', '#4c6c38', '#769456'],
    autumn: ['#3a4628', '#5a663a', '#808650'],
};

/**
 * Per season: the spike's height, its stalk colour, the number of open bells from its foot up, and
 * the number of buds above them, or of seed capsules once it has flowered. Winter has the leaf
 * rosette alone.
 */
const SPIKE: Record<Season, [height: number, stalk: string, bells: number, buds: number]> = {
    winter: [0, '#4a6038', 0, 0],
    spring: [0.75, '#5a7a44', 4, 7],
    summer: [1, '#5a7a44', 12, 0],
    autumn: [0.95, '#7a6448', 0, 10],
};

const [BELL, RIM, BUD, BLUSH, CAPSULE, HUSK] = ['#96307e', '#e88cc8', '#7a9a5a', '#c070a0', '#5a4430', '#8a6c48'].map(hex);

/**
 * Foxglove: a rosette of broad leaves, the inner ones raised, and one tall spike hung on one side
 * with nodding purple-pink bells. A bell is a three-sided trumpet that opens outward and down from
 * the stalk, pale at its mouth; the bells shrink up the spike to its tip. It builds itself 1 tall
 * in flower, so the winter rosette stays low.
 */
function foxglove(season: Season, rand: () => number): Geometry {
    const [height, stalk, bells, buds] = SPIKE[season];
    const colours = ramp(LEAVES[season]);
    const cards = sheet();
    for (let k = 0; k < 10; k++) {
        const inner = k >= 7, a = (inner ? (k - 6.5) / 3 : k / 7) * 2 * Math.PI + rand() * 0.5;
        const out = v3(Math.cos(a), Math.sin(a), 0), length = inner ? 0.2 + 0.04 * rand() : 0.3 + 0.07 * rand();
        const dir = out.clone().addScaledVector(UP, inner ? 0.9 : 0.22).normalize();
        leaf(cards, v3(), dir, length, 0.52 * length, 0.55, colours, (p: V) => out.clone().multiplyScalar((0.5 * p.length()) / length).add(UP).normalize());
    }
    if (!height) return cards.build();
    const face = rand() * 2 * Math.PI, lean = 0.05 * rand();
    const top = v3(lean * Math.cos(face), lean * Math.sin(face), height);
    const green = hex(stalk);
    spike(cards, v3(), top, 0.024, [green.clone().lerp(colours[0], 0.4), green]);
    const back = v3(-Math.cos(face), -Math.sin(face), 0.5).normalize();
    leaf(cards, top.clone().multiplyScalar(0.18), back, 0.17, 0.08, 0.5, colours);
    const count = bells + buds;
    for (let i = 0; i < count; i++) {
        const at = top.clone().multiplyScalar(0.36 + (0.58 * i) / (count - 1));
        // The bells keep to one side of the spike, a little spread round it.
        const a = face + (i % 2 ? 0.7 : -0.7) * (0.4 + 0.6 * rand());
        const out = v3(Math.cos(a), Math.sin(a), 0);
        const size = 0.15 * (1.1 - 0.55 * (i / count));
        if (i >= bells) {
            const seeds = season === 'autumn';
            leaf(cards, at, out.clone().addScaledVector(UP, seeds ? 0.7 : -0.25).normalize(), 0.55 * size, 0.3 * size, 0.5, seeds ? [CAPSULE, CAPSULE, HUSK] : [BUD, BUD, BLUSH]);
            continue;
        }
        const axis = out.clone().addScaledVector(UP, -0.8).normalize(), side = v3(-out.y, out.x, 0), under = v3().crossVectors(axis, side);
        const middle = at.clone().addScaledVector(axis, 0.6 * size);
        const mouth = [0, 1, 2, 0].map((k) => at.clone().addScaledVector(axis, size).addScaledVector(side, 0.4 * size * Math.sin((k * 2 * Math.PI) / 3)).addScaledVector(under, -0.4 * size * Math.cos((k * 2 * Math.PI) / 3)));
        const light = (p: V) => p.clone().sub(middle).normalize().lerp(UP, 0.45).normalize();
        for (let k = 0; k < 3; k++) cards.add([at, mouth[k], mouth[k + 1]], [BELL, RIM, RIM], light);
    }
    return cards.build();
}

export default {
    name: 'Foxglove',
    latin: 'Digitalis purpurea',
    build: (season, seed) => foxglove(season, random(seed)),
    ground: { on: ['grass', 'shrub'], height: [0.8, 1.5], share: 0.03, own: true, clumped: true },
    sized: true,
} satisfies Plant;
