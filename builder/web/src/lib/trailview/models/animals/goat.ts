import { rgb } from '../../structures/mesh';
import type { Animal } from '../animals';
import { both } from '../sculpt';
import { ears, eyes, HOOF, NOSE, quadruped, X, type Body } from './body/quadruped';

const BODY: Body = {
    trunk: [[-0.52, 0.76, 0.11, 0.11, 0.11], [-0.44, 0.75, 0.22, 0.22, 0.22], [-0.2, 0.74, 0.28, 0.25, 0.28], [0.1, 0.74, 0.28, 0.25, 0.29], [0.33, 0.75, 0.24, 0.24, 0.26], [0.45, 0.77, 0.16, 0.18, 0.19], [0.52, 0.78, 0.08, 0.09, 0.1]],
    neck: { at: [0.35, 0.83], length: 0.33, size: [[0.16, 0.18, 0.19], [0.12, 0.13, 0.13]], carry: [[58, 50], [-35, -60], [60, 50]] },
    head: { rows: [[0, 0.085, 0.085, 0.085], [0.07, 0.145, 0.135, 0.135], [0.2, 0.15, 0.135, 0.14], [0.34, 0.105, 0.095, 0.105], [0.43, 0.095, 0.085, 0.095], [0.47, 0.065, 0.055, 0.06]], nod: [-30, -75, -25], set: [0.06, 0.1] },
    front: { x: 0.14, y: 0.31, top: 0.58, thigh: 0.08, bone: 0.056 },
    hind: { x: 0.14, y: -0.37, top: 0.58, thigh: 0.092, bone: 0.056 },
};
const HORN = rgb('#8a7a64');

interface Coat { fore: string; rear: string; under: string; shin: string; muzzle: string }

export default {
    name: 'Goat',
    latin: 'Capra hircus',
    height: 0.8,
    // A black neck with a white rear; a dark brown; a chamois brown with dark legs; a white.
    coats: [
        { fore: '#2e241e', rear: '#ece4d4', under: '', shin: '', muzzle: '#2e241e' },
        { fore: '#4a3a2e', rear: '#4a3a2e', under: '#7a6048', shin: '#33271f', muzzle: '#8a6e54' },
        { fore: '#87482a', rear: '#87482a', under: '#e6dccb', shin: '#2e241e', muzzle: '#e6dccb' },
        { fore: '#ece4d4', rear: '#ece4d4', under: '#e0d6c2', shin: '#ece4d4', muzzle: '#e2c8bc' },
    ] as Coat[],
    poses: 3,
    build(s, coat, pose) {
        const [fore, rear] = [coat.fore, coat.rear].map(hex => rgb(hex)), under = coat.under ? rgb(coat.under) : 0, muzzle = rgb(coat.muzzle);
        const q = quadruped(s, BODY, pose, {
            trunk: (band, top) => (under && top < -0.6 ? under : band < 3 ? rear : fore), neck: fore, head: d => (d > 1 ? NOSE : d > 0.7 ? muzzle : fore),
            leg: front => { const c = front ? fore : rear; return [c, coat.shin ? rgb(coat.shin) : c, HOOF]; },
        }), h = q.head, k = q.crown(0.05, 0.1);
        ears(s, h, [0.11, 0.1, 0.04], [0.07, 0.34, -0.01], 0.11, fore);
        both(side => s.chain([k.p(0, side * 0.07, 0), k.p(-0.03, side * 0.09, 0.14), k.p(-0.12, side * 0.11, 0.24), k.p(-0.23, side * 0.14, 0.27)], [0.06, 0.05, 0.035, 0.012], HORN));
        eyes(s, h, [0.2, 0.135, 0.05]);
        s.leaf(h.p(0.35, 0, -0.08), h.p(0.32, 0, -0.3), 0.08, 0.05, X, muzzle);
        s.leaf(q.body([0, -0.44, 0.9]), q.body([0, -0.6, 1.08]), 0.09, 0.05, X, rear);
    },
} satisfies Animal<Coat>;
