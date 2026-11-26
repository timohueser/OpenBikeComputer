import { rgb } from '../../structures/mesh';
import type { Animal } from '../animals';
import { shade } from '../flat';
import { bellied, both } from '../sculpt';
import { ears, eyes, HOOF, NOSE, quadruped, X, type Body } from './body/quadruped';

const BODY: Body = {
    trunk: [[-0.56, 0.74, 0.12, 0.13, 0.13], [-0.47, 0.73, 0.25, 0.24, 0.24], [-0.22, 0.72, 0.31, 0.27, 0.3], [0.1, 0.72, 0.32, 0.28, 0.31], [0.36, 0.74, 0.29, 0.26, 0.29], [0.5, 0.76, 0.2, 0.2, 0.22], [0.57, 0.77, 0.1, 0.11, 0.12]],
    neck: { at: [0.38, 0.82], length: 0.3, size: [[0.21, 0.22, 0.24], [0.15, 0.16, 0.17]], carry: [[52, 42], [-35, -60], [56, 46]] },
    head: { rows: [[0, 0.095, 0.095, 0.095], [0.07, 0.155, 0.14, 0.14], [0.2, 0.155, 0.14, 0.145], [0.33, 0.115, 0.1, 0.115], [0.42, 0.105, 0.085, 0.1], [0.46, 0.07, 0.06, 0.065]], nod: [-28, -75, -22], set: [0.06, 0.1] },
    front: { x: 0.16, y: 0.34, top: 0.58, thigh: 0.095, bone: 0.065 },
    hind: { x: 0.165, y: -0.4, top: 0.58, thigh: 0.11, bone: 0.065 },
};
const HORN = rgb('#6e6152'), RIDGE = rgb('#8c7e6a');
// The sweep of a horn in the level frame of the crown: [back, up, out, width].
const SWEEP = [[0, 0, 0.06, 0.09], [0.04, 0.24, 0.09, 0.085], [0.15, 0.45, 0.13, 0.075], [0.31, 0.6, 0.17, 0.06], [0.5, 0.66, 0.21, 0.045], [0.66, 0.6, 0.24, 0.02]];

export default {
    name: 'Alpine ibex',
    latin: 'Capra ibex',
    height: 0.9,
    coats: ['#8f7a62', '#7a6652', '#a08a6c'].map(hex => rgb(hex)),
    poses: 3,
    build(s, coat, pose) {
        const dark = shade(coat, 0.55), pale = rgb('#d8cdb8');
        const q = quadruped(s, BODY, pose, { trunk: bellied(coat, pale), neck: coat, head: d => (d > 1 ? NOSE : d > 0.7 ? shade(coat, 1.2) : coat), leg: () => [coat, shade(coat, 0.7), HOOF] }), h = q.head, k = q.crown(0.06, 0.11);
        ears(s, h, [0.12, 0.12, 0.0], [0.08, 0.33, -0.05], 0.1, coat);
        eyes(s, h, [0.2, 0.14, 0.05]);
        both(side => {
            const at = SWEEP.map(([back, up, out]) => k.p(-back, side * out, up));
            s.chain(at, SWEEP.map(row => row[3]), HORN);
            // The knots on the front of the horn.
            s.fine(() => [1, 2, 3, 4].forEach(i => [0.15, 0.6].forEach(t => {
                const [back, up, out, w] = SWEEP[i].map((v, n) => v + (SWEEP[i + 1][n] - v) * t), turn = Math.atan2(SWEEP[i + 1][1] - SWEEP[i][1], SWEEP[i + 1][0] - SWEEP[i][0]);
                s.box(k.p(-back + 0.5 * w * Math.sin(turn), side * out, up + 0.5 * w * Math.cos(turn)), [1.25 * w, 0.045, 0.045], RIDGE);
            })));
        });
        s.leaf(h.p(0.33, 0, -0.08), h.p(0.31, 0, -0.24), 0.07, 0.05, X, dark);
        s.leaf(q.body([0, -0.48, 0.88]), q.body([0, -0.6, 0.8]), 0.1, 0.05, X, dark);
    },
} satisfies Animal<number>;
