import { rgb } from '../../structures/mesh';
import type { Animal } from '../animals';
import { shade } from '../flat';
import { bellied, both } from '../sculpt';
import { eyes, quadruped, type Body } from './body/quadruped';

// Lean and long: an arched back over slim legs, a long head that the pig carries low.
const BODY: Body = {
    trunk: [[-0.62, 0.64, 0.12, 0.14, 0.14], [-0.53, 0.65, 0.26, 0.27, 0.29], [-0.28, 0.67, 0.31, 0.31, 0.35], [0.06, 0.68, 0.31, 0.32, 0.36], [0.36, 0.67, 0.29, 0.3, 0.34], [0.54, 0.65, 0.23, 0.25, 0.27], [0.62, 0.64, 0.13, 0.15, 0.15]],
    neck: { at: [0.44, 0.64], length: 0.28, size: [[0.24, 0.27, 0.24], [0.18, 0.2, 0.18]], carry: [[-4, -8], [-30, -42], [6, 2]] },
    head: { rows: [[0, 0.12, 0.12, 0.11], [0.06, 0.18, 0.17, 0.15], [0.18, 0.165, 0.15, 0.14], [0.32, 0.1, 0.08, 0.095], [0.45, 0.085, 0.07, 0.085], [0.48, 0.08, 0.07, 0.08]], nod: [-20, -42, -12], set: [0.1, 0.03] },
    front: { x: 0.16, y: 0.4, top: 0.5, thigh: 0.09, bone: 0.055 },
    hind: { x: 0.165, y: -0.42, top: 0.5, thigh: 0.11, bone: 0.055 },
};
const SNOUT = rgb('#b89a94');

export default {
    name: 'Iberian pig',
    latin: 'Sus scrofa domesticus',
    height: 0.75,
    // Black, slate, and the dark red of the retinto.
    coats: ['#575250', '#6e6864', '#84583f'].map(hex => rgb(hex)),
    poses: 3,
    build(s, coat, pose) {
        const q = quadruped(s, BODY, pose, { trunk: bellied(coat, shade(coat, 0.85)), neck: coat, head: d => (d > 0.95 ? SNOUT : coat), leg: () => [coat, coat, shade(coat, 0.5)] }), h = q.head;
        // Ears that flop forward over the eyes.
        both(side => s.leaf(h.p(0.05, side * 0.11, 0.14), h.p(0.3, side * 0.22, 0.07), 0.2, 0.07, [0, 0, 1], shade(coat, 0.8)));
        eyes(s, h, [0.22, 0.135, 0.03], 0.04);
        s.fine(() => s.chain([[0, -0.62, 0.72], [0, -0.7, 0.66], [0, -0.7, 0.54]].map(q.body), [0.04, 0.035, 0.02], coat));
    },
} satisfies Animal<number>;
