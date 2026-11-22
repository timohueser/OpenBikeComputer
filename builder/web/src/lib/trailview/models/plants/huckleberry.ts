import type { Plant } from '../plant-library';
import { domed, GOLDEN, hex, leaf, merge, octahedron, paint, place, ramp, random, sheet, smooth, soft, spike, v3, type Geometry, type Ramp, type V } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** Huckleberry leaves: fresh green, scarlet in autumn, shed in winter. */
const HUCKLEBERRY: Record<Season, Ramp | 'bare'> = {
    winter: 'bare',
    spring: ['#4f8a3a', '#7ab84e', '#b0dc72'],
    summer: ['#3a6a2e', '#56883a', '#80aa50'],
    autumn: ['#7a1e22', '#a83228', '#d4583a'],
};

/** Stems of a bush, and the stems that carry a berry in summer. */
const STEMS = 6, BERRIES = 4;

/**
 * Huckleberry: an airy bush of six thin red stems that lean out from the root, each with a fork. Small oval
 * leaves stand in two rows along the stem and the fork, level like a fern's, and the bush lights as one round
 * tuft. Winter leaves the stems bare, each a broom of fine twigs; in summer a red berry hangs under the
 * fork of four stems. 128 triangles, 126 in winter, 160 in summer.
 */
function huckleberry(season: Season, rand: () => number): Geometry {
    const leaves = HUCKLEBERRY[season];
    const [wood, bark, shoot] = [hex('#5a2a24'), hex('#8a4434'), hex('#b0584a')];
    const [berry, shine] = [hex('#b81e22'), hex('#f07a66')];
    const light = domed(v3(0, 0, 0.2), v3(0.7, 0.7, 0.9), 0.35);
    const cards = sheet();
    const parts: Geometry[] = [];
    // The leaves draw from their own numbers, so the stems stand the same in every season.
    const vary = random(Math.floor(rand() * 1e6));
    const blade = (at: V, dir: V) => {
        if (leaves === 'bare') return;
        const [deep, mid, tip] = ramp(leaves);
        const body = mid.clone().lerp(tip, 0.8 * vary()).lerp(deep, 0.4 * (1 - smooth(0.2, 0.7, at.z)));
        leaf(cards, at, dir.clone().setZ(dir.z + 0.15).normalize(), 0.2 + 0.05 * vary(), 0.12, 0.5, [body.clone().lerp(deep, 0.5), body, body.clone().lerp(tip, 0.4)], light);
    };
    for (let k = 0; k < STEMS; k++) {
        const a = k * GOLDEN + rand() * 0.5;
        const out = v3(Math.cos(a), Math.sin(a), 0), side = v3(-out.y, out.x, 0);
        const tilt = 0.12 + 0.55 * Math.sqrt((k + 0.5) / STEMS);
        const length = (1 - (0.3 * k) / STEMS) * (0.85 + 0.15 * rand());
        const dir = out.clone().multiplyScalar(Math.sin(tilt)).setZ(Math.cos(tilt));
        const s = k % 2 ? 1 : -1;
        const on = (t: number) => dir.clone().multiplyScalar(t * length);
        const off = (to: number) => dir.clone().multiplyScalar(0.5).addScaledVector(side, to);
        const twig = (from: V, to: number, reach: number) => {
            const end = from.clone().addScaledVector(dir.clone().addScaledVector(side, to).normalize(), reach * length);
            spike(cards, from, end, 0.018, [bark, shoot]);
            return end;
        };
        spike(cards, v3(), on(1), 0.035, [wood, bark]);
        const fork = twig(on(0.4), 0.9 * s, 0.5);
        if (leaves === 'bare') {
            // Fine twigs stand up beside the stem and the fork, as a broom.
            [0.5, 0.64, 0.78].forEach((t, i) => twig(on(t), (i % 2 ? 0.45 : -0.45) * s, 0.62 * (1 - t) + 0.08));
            [0.35, 0.65].forEach((t, i) => twig(on(0.4).lerp(fork, t), i ? 0.2 * s : 1.5 * s, 0.26 - 0.08 * i));
            continue;
        }
        [0.5, 0.62, 0.74, 0.86].slice(k < 4 ? 0 : 1).forEach((t, i) => blade(on(t), off(i % 2 ? s : -s)));
        blade(on(1), dir);
        const along = fork.clone().sub(on(0.4)).normalize();
        [0.4, 0.7].forEach((t, i) => blade(on(0.4).lerp(fork, t), along.clone().multiplyScalar(0.5).addScaledVector(dir, i ? -0.8 : 0.8)));
        blade(fork, along);
        if (season === 'summer' && k < BERRIES) {
            const hang = on(0.4).lerp(fork, 0.55);
            parts.push(paint(place(soft(octahedron(0.05)), { at: [hang.x, hang.y, hang.z - 0.045] }), (p) => berry.clone().lerp(shine, smooth(hang.z - 0.06, hang.z, p.z))));
        }
    }
    parts.push(cards.build());
    return merge(parts);
}

export default {
    name: 'Huckleberry',
    latin: 'Vaccinium parvifolium',
    build: (season, seed) => huckleberry(season, random(seed)),
    ground: { on: ['tree'], height: [0.4, 0.8], share: 0.25 },
} satisfies Plant;
