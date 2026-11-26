import { rgb } from '../../structures/mesh';
import type { Animal } from '../animals';
import { shade } from '../flat';
import { both } from '../sculpt';
import { ears, eyes, HOOF, NOSE, quadruped, X, type Body } from './body/quadruped';

const BODY: Body = {
    trunk: [[-0.5, 0.79, 0.1, 0.11, 0.11], [-0.42, 0.78, 0.21, 0.2, 0.21], [-0.18, 0.77, 0.26, 0.23, 0.26], [0.1, 0.77, 0.26, 0.23, 0.27], [0.31, 0.78, 0.23, 0.22, 0.24], [0.43, 0.8, 0.16, 0.17, 0.18], [0.5, 0.81, 0.08, 0.09, 0.1]],
    neck: { at: [0.32, 0.86], length: 0.32, size: [[0.16, 0.18, 0.19], [0.115, 0.125, 0.13]], carry: [[66, 58], [-35, -60], [66, 56]] },
    head: { rows: [[0, 0.08, 0.085, 0.085], [0.06, 0.135, 0.125, 0.125], [0.18, 0.14, 0.125, 0.13], [0.31, 0.1, 0.09, 0.1], [0.39, 0.09, 0.075, 0.085], [0.43, 0.06, 0.05, 0.055]], nod: [-24, -75, -20], set: [0.05, 0.09] },
    front: { x: 0.13, y: 0.29, top: 0.62, thigh: 0.08, bone: 0.052 },
    hind: { x: 0.135, y: -0.35, top: 0.62, thigh: 0.092, bone: 0.052 },
};
const HORN = rgb('#231d1a'), FACE = rgb('#eadfc8');

export default {
    name: 'Chamois',
    latin: 'Rupicapra rupicapra',
    height: 0.78,
    // The dark coat of winter, the brown of autumn, the fawn of summer.
    coats: ['#5c463a', '#6e5038', '#9c7448'].map(hex => rgb(hex)),
    poses: 3,
    build(s, coat, pose) {
        const dark = shade(coat, 0.5);
        const q = quadruped(s, BODY, pose, {
            trunk: (band, top) => (band < 0 ? FACE : top < -0.6 ? shade(FACE, 0.9) : top > 0.9 ? dark : coat), neck: (_, top) => (top < -0.6 ? FACE : coat),
            // A pale face with a dark stripe from the ear through the eye to the nose.
            head: (d, top) => (d > 1 ? NOSE : d < 0.2 ? coat : Math.abs(top) < 0.3 ? dark : FACE), leg: () => [coat, shade(coat, 0.6), HOOF],
        }), h = q.head, k = q.crown(0.07, 0.1);
        ears(s, h, [0.1, 0.1, 0.03], [0.02, 0.24, 0.16], 0.09, coat);
        eyes(s, h, [0.19, 0.125, 0.045], 0.045);
        both(side => s.chain([k.p(0, side * 0.045, 0), k.p(0.03, side * 0.06, 0.2), k.p(-0.02, side * 0.075, 0.28), k.p(-0.08, side * 0.085, 0.24)], [0.048, 0.04, 0.032, 0.012], HORN));
        s.leaf(q.body([0, -0.44, 0.9]), q.body([0, -0.55, 0.82]), 0.08, 0.05, X, dark);
    },
} satisfies Animal<number>;
