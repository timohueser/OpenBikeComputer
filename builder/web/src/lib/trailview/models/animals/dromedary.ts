import { rgb } from '../../structures/mesh';
import type { Animal } from '../animals';
import { shade } from '../flat';
import { alongY, bellied } from '../sculpt';
import { ears, eyes, quadruped, type Body } from './body/quadruped';

const BODY: Body = {
    trunk: [[-0.6, 0.88, 0.09, 0.1, 0.1], [-0.52, 0.86, 0.2, 0.15, 0.18], [-0.28, 0.85, 0.25, 0.17, 0.22], [0.04, 0.84, 0.26, 0.17, 0.25], [0.32, 0.85, 0.23, 0.16, 0.25], [0.48, 0.85, 0.17, 0.14, 0.2], [0.56, 0.85, 0.09, 0.09, 0.12]],
    neck: { at: [0.42, 0.78], length: 0.86, size: [[0.15, 0.17, 0.2], [0.1, 0.11, 0.13], [0.085, 0.09, 0.1], [0.08, 0.085, 0.09]], carry: [[8, 92], [-50, -20], [30, 95]] },
    head: { rows: [[0, 0.075, 0.075, 0.075], [0.06, 0.125, 0.11, 0.11], [0.18, 0.125, 0.11, 0.115], [0.32, 0.1, 0.075, 0.1], [0.43, 0.095, 0.065, 0.095], [0.47, 0.065, 0.045, 0.065]], nod: [-12, -50, -10], set: [0.1, -0.02] },
    front: { x: 0.14, y: 0.36, top: 0.8, thigh: 0.085, bone: 0.052 },
    hind: { x: 0.14, y: -0.42, top: 0.8, thigh: 0.1, bone: 0.052 },
};

export default {
    name: 'Dromedary',
    latin: 'Camelus dromedarius',
    height: 1.85,
    coats: ['#c9a877', '#b08d60', '#dcc6a0'].map(hex => rgb(hex)),
    poses: 3,
    build(s, coat, pose) {
        const dark = shade(coat, 0.8);
        const q = quadruped(s, BODY, pose, { trunk: bellied(coat, shade(coat, 0.88)), neck: coat, head: d => (d > 1 ? shade(coat, 0.6) : coat), leg: () => [coat, coat, shade(coat, 0.7)] }), h = q.head;
        const [, , top] = q.body([0, 0, 0.98]);
        s.skin(alongY([[-0.46, top - 0.02, 0.1, 0.05, 0.1], [-0.3, top + 0.03, 0.2, 0.15, 0.12], [-0.1, top + 0.05, 0.22, 0.21, 0.14], [0.1, top + 0.03, 0.18, 0.14, 0.12], [0.26, top - 0.02, 0.09, 0.05, 0.1]]), 8, (_, up) => (up > 0.9 ? dark : coat));
        ears(s, h, [0.06, 0.09, 0.05], [0.0, 0.17, 0.12], 0.07, coat);
        eyes(s, h, [0.17, 0.11, 0.04], 0.045);
        s.fine(() => s.chain([[0, -0.6, 0.94], [0, -0.68, 0.74], [0, -0.67, 0.56]].map(q.body), [0.04, 0.035, 0.05], dark));
    },
} satisfies Animal<number>;
