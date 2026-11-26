import { rgb } from '../../structures/mesh';
import type { Animal } from '../animals';
import { shade } from '../flat';
import { bellied, both } from '../sculpt';
import { ears, HOOF, HORN, NOSE, quadruped, type Body } from './body/quadruped';

// A low, wide trunk under long hair that hangs to the knees.
const BODY: Body = {
    trunk: [[-0.64, 0.7, 0.14, 0.16, 0.16], [-0.54, 0.68, 0.3, 0.29, 0.3], [-0.28, 0.67, 0.37, 0.32, 0.37], [0.06, 0.67, 0.38, 0.33, 0.39], [0.36, 0.69, 0.35, 0.31, 0.36], [0.54, 0.71, 0.26, 0.25, 0.28], [0.62, 0.72, 0.14, 0.15, 0.16]],
    neck: { at: [0.4, 0.74], length: 0.36, size: [[0.25, 0.24, 0.27], [0.18, 0.18, 0.2]], carry: [[18, 12], [-30, -55], [28, 18]] },
    head: { rows: [[0, 0.12, 0.11, 0.11], [0.07, 0.19, 0.17, 0.17], [0.2, 0.19, 0.17, 0.18], [0.33, 0.15, 0.12, 0.14], [0.42, 0.14, 0.11, 0.13], [0.48, 0.1, 0.08, 0.09]], nod: [-34, -72, -28] },
    front: { x: 0.19, y: 0.38, top: 0.56, thigh: 0.12, bone: 0.085 },
    hind: { x: 0.2, y: -0.42, top: 0.56, thigh: 0.135, bone: 0.085 },
};

export default {
    name: 'Highland cow',
    latin: 'Bos taurus',
    height: 1.2,
    coats: [['#b3561f', '#e09048'], ['#4a2a1a', '#7a4a2c'], ['#d0ae74', '#efd6a2']].map(pair => pair.map(hex => rgb(hex))),
    poses: 3,
    build(s, [coat, light], pose) {
        const q = quadruped(s, BODY, pose, { trunk: bellied(coat, shade(coat, 0.8)), neck: coat, head: d => (d > 1 ? NOSE : d > 0.8 ? light : coat), leg: () => [coat, coat, HOOF] }), h = q.head;
        // The fringe over the eyes, and hair that hangs from the neck and the flanks.
        s.skin(h.rings([[-0.04, 0.12, 0.08, 0.06], [0.1, 0.2, 0.14, 0.05], [0.27, 0.15, 0.16, 0.03], [0.34, 0.07, 0.16, 0.02]].map(([d, w, up, down]) => [d, w, up + 0.06, down - 0.06])), 8, light);
        s.fine(() => s.leaf(q.neck(0.25, -0.2), q.neck(0.6, -0.42), 0.26, 0.12, [1, 0, 0], shade(coat, 0.9)));
        ears(s, h, [0.08, 0.16, 0.0], [0.03, 0.42, -0.08], 0.15, coat);
        const k = q.crown(0.07, 0.13);
        both(side => s.chain([k.p(0, side * 0.1, 0), k.p(0.02, side * 0.34, 0.03), k.p(0.07, side * 0.56, 0.17), k.p(0.1, side * 0.62, 0.4)], [0.07, 0.06, 0.045, 0.012], HORN));
        s.fine(() => s.chain([[0, -0.64, 0.86], [0, -0.74, 0.55], [0, -0.74, 0.22]].map(q.body), [0.05, 0.05, 0.1], coat));
    },
} satisfies Animal<number[]>;
