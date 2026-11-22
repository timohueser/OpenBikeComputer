import { rgb } from '../../structures/mesh';
import type { Animal } from '../animals';
import { shade } from '../flat';
import { ears, eyes, quadruped, type Body } from './body/quadruped';

const BODY: Body = {
    trunk: [[-0.6, 0.78, 0.1, 0.12, 0.12], [-0.52, 0.76, 0.23, 0.22, 0.22], [-0.28, 0.75, 0.28, 0.24, 0.26], [0.04, 0.74, 0.29, 0.24, 0.28], [0.3, 0.76, 0.25, 0.23, 0.25], [0.45, 0.78, 0.19, 0.2, 0.21], [0.52, 0.79, 0.1, 0.11, 0.12]],
    neck: { at: [0.36, 0.82], length: 0.44, size: [[0.17, 0.2, 0.22], [0.12, 0.15, 0.16], [0.105, 0.13, 0.13]], carry: [[42, 30], [-28, -52], [48, 36]] },
    head: { rows: [[0, 0.085, 0.09, 0.09], [0.07, 0.14, 0.135, 0.135], [0.21, 0.145, 0.135, 0.145], [0.37, 0.115, 0.1, 0.12], [0.48, 0.11, 0.09, 0.11], [0.53, 0.08, 0.06, 0.075]], nod: [-42, -70, -35], set: [0.08, 0.04] },
    front: { x: 0.14, y: 0.33, top: 0.66, thigh: 0.085, bone: 0.055 },
    hind: { x: 0.15, y: -0.42, top: 0.66, thigh: 0.1, bone: 0.055 },
};
const PALE = rgb('#ddd4c4');

export default {
    name: 'Donkey',
    latin: 'Equus asinus',
    height: 1.1,
    coats: ['#9a958e', '#8a705a', '#6c6058'].map(hex => rgb(hex)),
    poses: 3,
    build(s, coat, pose) {
        const dark = shade(coat, 0.45);
        const q = quadruped(s, BODY, pose, {
            // A pale belly, and the dark stripe of the back.
            trunk: (_, top) => (top < -0.6 ? PALE : top > 0.9 ? shade(coat, 0.7) : coat), neck: coat,
            head: d => (d > 1 ? shade(PALE, 0.75) : d > 0.8 ? PALE : coat), leg: () => [coat, coat, rgb('#2a2624')],
        }), h = q.head;
        // Long ears, with a pale ring round each eye.
        ears(s, h, [0.06, 0.08, 0.1], [-0.06, 0.17, 0.42], 0.13, coat);
        s.fine(() => [-1, 1].forEach(side => s.box(h.p(0.21, side * 0.14, 0.05), [0.03, 0.085, 0.085], PALE)));
        eyes(s, h, [0.21, 0.15, 0.05]);
        s.chain([q.neck(0.15, 0.2), q.neck(0.55, 0.17), q.neck(0.98, 0.15)], [0.06, 0.07, 0.06], dark);
        s.fine(() => s.chain([[0, -0.6, 0.9], [0, -0.7, 0.7], [0, -0.71, 0.48]].map(q.body), [0.045, 0.04, 0.04], coat));
        s.chain([q.body([0, -0.71, 0.5]), q.body([0, -0.7, 0.28])], [0.09, 0.04], dark);
    },
} satisfies Animal<number>;
