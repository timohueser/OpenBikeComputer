import type { Plant } from '../plant-library';
import { domed, GOLDEN, hex, leaf, ramp, random, sheet, spike, UP, v3, type Geometry, type Ramp, type V } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** Dwarf birch leaves: dark glossy green, rust-red to orange in autumn, shed in winter. */
const DWARF_BIRCH: Record<Season, Ramp | 'bare'> = {
    winter: 'bare',
    spring: ['#34602a', '#52863a', '#84b058'],
    summer: ['#24401e', '#3a5c2c', '#5e8040'],
    autumn: ['#5c1a14', '#9a381e', '#c8602c'],
};

const STEMS = 8;

/**
 * Dwarf birch: a low, spreading thicket about three times as wide as tall, eight stiff dark stems that lean out
 * from the root, each with two forks. Small round leaves sit along the stems and the forks, and the thicket lights
 * as one low mound. Winter leaves the twigs bare, with many short side twigs.
 */
function dwarfBirch(season: Season, rand: () => number): Geometry {
    const leaves = DWARF_BIRCH[season];
    const [wood, bark] = [hex('#2e1e1c'), hex('#5a3a30')];
    const light = domed(v3(0, 0, -0.1), v3(1.4, 1.4, 1), 0.4);
    const cards = sheet();
    // The leaves draw from their own numbers, so the stems stand the same in every season.
    const vary = random(Math.floor(rand() * 1e6));
    for (let k = 0; k < STEMS; k++) {
        const a = k * GOLDEN + rand() * 0.5;
        const out = v3(Math.cos(a), Math.sin(a), 0), side = v3(-out.y, out.x, 0);
        const tilt = 0.15 + (1.05 * (k + 0.5)) / STEMS;
        const length = 1 + 0.3 * rand();
        const dir = out.clone().multiplyScalar(Math.sin(tilt)).setZ(Math.cos(tilt));
        const s = k % 2 ? 1 : -1;
        const on = (t: number) => dir.clone().multiplyScalar(t * length);
        const twig = (from: V, way: V, reach: number) => {
            const end = from.clone().addScaledVector(way.normalize(), reach * length);
            spike(cards, from, end, 0.018, [bark, bark]);
            return end;
        };
        const fork = (t: number, to: number, reach: number) => twig(on(t), dir.clone().addScaledVector(side, 0.8 * to).addScaledVector(UP, 0.5), reach);
        // A round leaf lies out from its twig, a little raised, to side `to`.
        const blade = (at: V, to: number) => {
            if (leaves === 'bare') return;
            const [deep, mid, tip] = ramp(leaves);
            const body = mid.clone().lerp(tip, 0.8 * vary()).lerp(deep, 0.35 * vary());
            leaf(cards, at, out.clone().addScaledVector(side, to).setZ(0.5).normalize(), 0.32 + 0.06 * vary(), 0.27, 0.42, [body.clone().lerp(deep, 0.6), body, body.clone().lerp(tip, 0.3)], light);
        };
        spike(cards, v3(), on(1), 0.04, [wood, bark]);
        const forks = [fork(0.35, s, 0.5), fork(0.6, -s, 0.36)];
        if (leaves === 'bare') {
            [0.22, 0.48, 0.72, 0.86].forEach((t, i) => fork(t, i % 2 ? -0.6 * s : 0.6 * s, 0.4 * (1 - t) + 0.12));
            forks.forEach((end, i) => twig(on(i ? 0.6 : 0.35).lerp(end, 0.5), dir.clone().addScaledVector(side, i ? 0.4 * s : -0.4 * s), 0.26));
            continue;
        }
        [0.5, 0.76, 1].forEach((t, i) => blade(on(t), i % 2 ? s : -s));
        forks.forEach((end, i) => [0.45, 0.75, 1].forEach((t, j) => blade(on(i ? 0.6 : 0.35).lerp(end, t), (i ? -s : s) * (j % 2 ? 1.6 : 0.4))));
    }
    return cards.build();
}

export default {
    name: 'Dwarf birch',
    latin: 'Betula nana',
    build: (season, seed) => dwarfBirch(season, random(seed)),
    ground: { on: ['shrub', 'moss', 'grass'], height: [0.2, 0.5], share: 0.3, own: true },
} satisfies Plant;
