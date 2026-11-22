import type { Plant } from '../plant-library';
import { domed, GOLDEN, hex, leaf, ramp, random, sheet, smooth, spike, spindle, v3, type Geometry, type Ramp, type V } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** Bog myrtle leaves: grey-green, dull olive-yellow in autumn, shed in winter. */
const MYRTLE: Record<Season, Ramp | 'bare'> = {
    winter: 'bare',
    spring: ['#40503a', '#68785a', '#9aa686'],
    summer: ['#3c4a36', '#64725a', '#98a488'],
    autumn: ['#585230', '#867c46', '#ae9e62'],
};

const CATKIN = ['#7a4426', '#b06a3a'].map(hex);

const STEMS = 9;

/**
 * Bog myrtle: an upright broom of nine stiff red-brown stems, a little taller than wide, each with a fork. Narrow
 * leaves, widest near their tips, stand up round the stems. In spring an orange-brown catkin ends each stem before
 * half of the leaves are out; winter leaves the twigs bare, with more forks and the small catkin buds. 180
 * triangles.
 */
function bogMyrtle(season: Season, rand: () => number): Geometry {
    const leaves = MYRTLE[season];
    const [wood, bark] = [hex('#3a2420'), hex('#7a4634')];
    const light = domed(v3(0, 0, 0.3), v3(0.5, 0.5, 0.8), 0.35);
    const cards = sheet();
    // The leaves draw from their own numbers, so the stems stand the same in every season.
    const vary = random(Math.floor(rand() * 1e6));
    const catkin = (at: V, dir: V, size: number) => spindle(cards, at, at.clone().addScaledVector(dir, 0.17 * size), 0.045 * size, 3, CATKIN);
    for (let k = 0; k < STEMS; k++) {
        const a = k * GOLDEN + rand() * 0.5;
        const out = v3(Math.cos(a), Math.sin(a), 0), side = v3(-out.y, out.x, 0);
        const tilt = 0.06 + 0.42 * Math.sqrt((k + 0.5) / STEMS);
        const length = (1 - (0.2 * k) / STEMS) * (0.86 + 0.12 * rand());
        const dir = out.clone().multiplyScalar(Math.sin(tilt)).setZ(Math.cos(tilt));
        const s = k % 2 ? 1 : -1;
        const on = (t: number) => dir.clone().multiplyScalar(t * length);
        const twig = (t: number, to: number, reach: number) => {
            const way = dir.clone().addScaledVector(side, 0.7 * to).normalize(), end = on(t).addScaledVector(way, reach * length);
            spike(cards, on(t), end, 0.02, [bark, bark]);
            return [end, way];
        };
        // Leaf `i` of a twig stands up from it, to the next side round it.
        const blade = (at: V, way: V, i: number) => {
            if (leaves === 'bare') return;
            const [deep, mid, tip] = ramp(leaves);
            const [u, w] = [side, v3().crossVectors(way, side)], t = (i + 3 * k) * GOLDEN;
            const body = mid.clone().lerp(tip, 0.7 * vary()).lerp(deep, 0.4 * (1 - smooth(0.3, 0.8, at.z)));
            leaf(cards, at, way.clone().addScaledVector(u, 0.65 * Math.cos(t)).addScaledVector(w, 0.65 * Math.sin(t)).normalize(), 0.25 + 0.05 * vary(), 0.13, 0.65, [deep, body, body.clone().lerp(tip, 0.4)], light);
        };
        spike(cards, v3(), on(1), 0.032, [wood, bark]);
        const [fork, way] = twig(0.45, s, 0.42);
        if (leaves === 'bare') {
            const [second] = twig(0.62, -s, 0.3);
            twig(0.78, s, 0.2);
            catkin(on(1), dir, 0.7);
            if (k % 3 === 0) catkin(k % 2 ? second : fork, dir, 0.7);
            continue;
        }
        const early = season === 'spring';
        if (early) catkin(on(1), dir, 1);
        else blade(on(1), dir, 0);
        (early ? [0.6, 0.82] : [0.5, 0.66, 0.82]).forEach((t, i) => blade(on(t), dir, i + 1));
        (early ? [0.6, 1] : [0.4, 0.75, 1]).forEach((t, i) => blade(on(0.45).lerp(fork, t), way, i + 4));
    }
    return cards.build();
}

export default {
    name: 'Bog myrtle',
    latin: 'Myrica gale',
    build: (season, seed) => bogMyrtle(season, random(seed)),
    ground: { on: ['bog', 'marsh'], height: [0.5, 1.2], share: 0.15, own: true, clumped: true },
} satisfies Plant;
