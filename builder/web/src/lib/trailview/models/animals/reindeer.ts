import { rgb } from '../../structures/mesh';
import type { Animal } from '../animals';
import { shade } from '../flat';
import { antlers, DEER } from './body/deer';
import { ears, eyes, HOOF, NOSE, quadruped, X, type Body } from './body/quadruped';

// A stocky deer on shorter legs that carries its head low.
const BODY: Body = {
    ...DEER,
    trunk: DEER.trunk.map(([y, z, w, up, down]) => [1.05 * y, z - 0.03, 1.16 * w, up + 0.03, 1.15 * down]),
    neck: { at: [0.36, 0.82], length: 0.38, size: [[0.19, 0.2, 0.24], [0.14, 0.15, 0.19], [0.12, 0.13, 0.15]], carry: [[34, 22], [-35, -55], [40, 28]] },
    head: { rows: [[0, 0.085, 0.085, 0.085], [0.06, 0.14, 0.125, 0.125], [0.18, 0.14, 0.125, 0.13], [0.31, 0.11, 0.09, 0.105], [0.4, 0.105, 0.08, 0.095], [0.45, 0.075, 0.055, 0.065]], nod: [-30, -72, -24], set: [0.07, 0.05] },
    front: { x: 0.15, y: 0.32, top: 0.62, thigh: 0.085, bone: 0.055 },
    hind: { x: 0.155, y: -0.4, top: 0.62, thigh: 0.1, bone: 0.055 },
};
const PALE = rgb('#ddd5c4'), ANTLER = rgb('#b9a88c');

export default {
    name: 'Reindeer',
    latin: 'Rangifer tarandus',
    height: 1.1,
    coats: ['#6e5e50', '#857361', '#a39584'].map(hex => rgb(hex)),
    poses: 3,
    build(s, coat, pose) {
        const dark = shade(coat, 0.8);
        const q = quadruped(s, BODY, pose, {
            trunk: (band, top) => (band < 1 ? PALE : top < -0.6 ? shade(PALE, 0.9) : band > 3 ? shade(coat, 1.15) : coat), neck: PALE,
            head: d => (d > 1 ? NOSE : d > 0.75 ? PALE : coat), leg: () => [coat, dark, HOOF],
        }), h = q.head;
        ears(s, h, [0.09, 0.12, 0.03], [0.04, 0.3, 0.06], 0.1, coat);
        eyes(s, h, [0.19, 0.14, 0.045], 0.045);
        antlers(s, q.crown(0.06, 0.1), [[0, 0, 0.07], [0.22, 0.24, 0.2], [0.36, 0.52, 0.3], [0.2, 0.78, 0.34], [0.02, 0.86, 0.32]], [[0, -0.2, 0.1, 0.05], [3, 0.28, 0.96, 0.4], [1, 0.34, 0.2, 0.26], [2, 0.5, 0.56, 0.36]], ANTLER, 0.05);
        // The mane under the neck.
        s.leaf(q.neck(0.15, -0.2), q.neck(0.7, -0.34), 0.2, 0.14, X, PALE);
        s.leaf(q.body([0, -0.52, 0.9]), q.body([0, -0.62, 0.8]), 0.09, 0.05, X, PALE);
    },
} satisfies Animal<number>;
