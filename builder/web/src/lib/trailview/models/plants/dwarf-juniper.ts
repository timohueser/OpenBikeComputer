import type { Plant } from '../plant-library';
import { cushion, hex, merge, paint, place, ramp, random, sheet, smooth, soft, spike, spindle, tetrahedron, UP, v3, type Geometry, type Ramp } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** Dwarf juniper needles: grey-green, a little bronzed in winter. */
const JUNIPER: Record<Season, Ramp> = {
    winter: ['#33362c', '#5a604e', '#909682'],
    spring: ['#2a3e32', '#56725a', '#98b294'],
    summer: ['#2a3e32', '#54705a', '#92ae92'],
    autumn: ['#2c3c32', '#526a56', '#8ca48a'],
};

/** The pale band on each needle, which greys the boughs where the light catches them. */
const BLOOM = hex('#b4c4bc');
const BERRY = hex('#3a4458');

/**
 * A dwarf juniper: a low plate about three and a half times as wide as tall. Plump needle boughs lie out from a
 * dark heart over the ground, their tips turned up, and shorter ones rise from its middle; each bough has a prickly
 * side shoot, and three blue-black berries sit on the inner boughs.
 */
function dwarfJuniper(season: Season, seed: number, rand: () => number): Geometry {
    const [deep, mid, tip] = ramp(JUNIPER[season]);
    const cards = sheet();
    const parts = [cushion([0, 0, 0.95, 0.62], 9, [deep, deep.clone().lerp(mid, 0.6), mid.clone().lerp(tip, 0.3)], 0.4, seed)];
    const turn = rand() * 2 * Math.PI;
    /** Bough `j` of a ring of `of`, from `from` out to `to` (each a distance from the middle and a height). */
    const bough = (j: number, of: number, from: [number, number], to: [number, number]) => {
        const a = turn + ((j + 0.6 * rand()) / of) * 2 * Math.PI, out = v3(Math.cos(a), Math.sin(a), 0);
        const [base, end] = [from, to].map(([d, z]) => out.clone().multiplyScalar(d).setZ(z));
        const needles = mid.clone().lerp(tip, 0.2 + 0.6 * rand()).lerp(BLOOM, 0.2 * rand());
        spindle(cards, base, end, 0.23, 4, [deep, needles]);
        // The side shoot leaves the bough's thick part, forward and to one side.
        const side = v3(-out.y, out.x, 0).multiplyScalar(j % 2 ? 0.8 : -0.8).add(out).addScaledVector(UP, 0.5).normalize();
        const foot = base.clone().lerp(end, 0.45);
        spike(cards, foot, foot.clone().addScaledVector(side, 0.5 + 0.1 * rand()), 0.1, [deep.clone().lerp(mid, 0.5), needles.clone().lerp(BLOOM, 0.15)]);
        return foot;
    };
    for (let j = 0; j < 9; j++) bough(j, 9, [0.45, 0.14], [1.5 + 0.3 * rand(), 0.32 + 0.2 * rand()]);
    for (let j = 0; j < 6; j++) {
        const at = bough(j, 6, [0.1, 0.3], [0.75 + 0.2 * rand(), 0.8 + 0.15 * rand()]);
        if (j % 2) continue;
        const ball = place(soft(tetrahedron(0.12)), { at: [at.x, at.y, at.z + 0.2], rot: [0, 0, rand() * Math.PI] });
        parts.push(paint(ball, (p) => BERRY.clone().lerp(BLOOM, 0.5 * smooth(at.z + 0.18, at.z + 0.28, p.z))));
    }
    return merge([...parts, cards.build()]);
}

export default {
    name: 'Dwarf juniper',
    latin: 'Juniperus communis subsp. nana',
    build: (season, seed) => dwarfJuniper(season, seed, random(seed)),
    ground: { on: ['shrub', 'grass', 'bare', 'moss'], height: [0.3, 0.6], share: 0.08, own: true, clumped: true },
} satisfies Plant;
