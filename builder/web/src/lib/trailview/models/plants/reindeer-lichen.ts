import type { Plant } from '../plant-library';
import { hash, hex, lathe, merge, paint, place, ramp, random, smooth, soft, tufts, type Geometry, type Ramp, type Tuft } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** Reindeer lichen: grey with a green cast while wet in spring and autumn, cream-white when dry in summer. */
const LICHEN: Record<Season, Ramp> = {
    winter: ['#7e8278', '#aeb0a2', '#d2d2c4'],
    spring: ['#7c8474', '#aab09a', '#cfd2bc'],
    summer: ['#a29c86', '#d2ccb4', '#f0ead4'],
    autumn: ['#828472', '#b2b29a', '#d6d4bc'],
};

/**
 * A flat mat of low rounded `cushions` grown into each other on an irregular outline, each
 * `lift` × (0.08 + 0.06 of its height) tall. Each cushion is an eight-sided dome of 24 triangles,
 * lit as its own mound, so the creases where two meet shade the mat into cushions; `share` of the
 * vertices take the `fleck` colour.
 */
export function cushionMat(cushions: Tuft[], colours: Ramp, [fleck, share]: [string, number], lift: number, seed: number, rand: () => number): Geometry {
    const [deep, mid, tip] = ramp(colours);
    const dots = hex(fleck);
    const parts = cushions.map(([x, y, r, h], k) => {
        const z = lift * (0.08 + 0.06 * h);
        const tone = 0.15 * (rand() - 0.5);
        // Bumps in the rim make the outline knobbly, not a polygon; the dome sinks a little, so they never lift it.
        const dome = soft(lathe([[r, 0], [0.8 * r, 0.6 * z], [0.4 * r, 0.95 * z], [0, z]], 8, 2), 0.05, 7, seed + k);
        return paint(place(dome, { at: [x, y, -0.02], rot: [0, 0, rand() * Math.PI] }), (p) => {
            const c = deep.clone().lerp(mid, smooth(0, 0.5, p.z / z)).lerp(tip, smooth(0.4, 1, p.z / z) + tone);
            return hash(p, seed) < share ? c.lerp(dots, 0.35) : c;
        });
    });
    return merge(parts);
}

export default {
    name: 'Reindeer lichen',
    latin: 'Cladonia rangiferina, Cladonia stellaris',
    // A mat twelve times as wide as tall, of seven cushions with the brown-grey tips of Cladonia
    // rangiferina: 168 triangles.
    build: (season, seed) => {
        const rand = random(seed);
        return cushionMat(tufts([0, 0, 0.6, 1], 6, 0.6, [0.6, 0.45, 0.5], [0.25, 0.15, 0.5], rand), LICHEN[season], ['#857a64', 0.15], 1, seed, rand);
    },
    ground: { on: ['moss', 'bare', 'shrub'], height: [0.05, 0.1], share: 0.2, own: true, clumped: true },
} satisfies Plant;
