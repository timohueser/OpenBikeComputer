import { rgb } from '../../structures/mesh';
import type { Animal } from '../animals';
import { shade } from '../flat';
import { axis, bellied } from '../sculpt';
import { ears, eyes, quadruped, type Body } from './body/quadruped';

// Few, large lumps of wool: a trunk through 5 rings.
const BODY: Body = {
    trunk: [[-0.56, 0.71, 0.17, 0.16, 0.16], [-0.4, 0.7, 0.35, 0.31, 0.33], [-0.03, 0.69, 0.39, 0.33, 0.36], [0.34, 0.71, 0.34, 0.3, 0.33], [0.5, 0.74, 0.17, 0.16, 0.17]],
    wool: 0.05,
    neck: { at: [0.34, 0.8], length: 0.3, size: [[0.21, 0.21, 0.21], [0.16, 0.16, 0.16]], carry: [[36, 30], [-30, -55], [42, 34]] },
    head: { rows: [[0, 0.08, 0.08, 0.08], [0.06, 0.13, 0.12, 0.12], [0.18, 0.135, 0.12, 0.12], [0.3, 0.1, 0.09, 0.1], [0.38, 0.09, 0.075, 0.085], [0.42, 0.06, 0.05, 0.055]], nod: [-35, -75, -28], set: [0.09, 0.03] },
    front: { x: 0.17, y: 0.3, top: 0.55, thigh: 0.07, bone: 0.052 },
    hind: { x: 0.17, y: -0.36, top: 0.55, thigh: 0.075, bone: 0.052 },
};

export default {
    name: 'Sheep',
    latin: 'Ovis aries',
    height: 0.8,
    // Wool and face: two with a black face, a white one, a brown one.
    coats: [['#efe7d6', '#2d2828'], ['#e6dcc6', '#2d2828'], ['#efe9dc', '#dccdb6'], ['#6a5444', '#3a2c26']].map(pair => pair.map(hex => rgb(hex))),
    poses: 3,
    build(s, [wool, face], pose) {
        const q = quadruped(s, BODY, pose, { trunk: bellied(wool, shade(wool, 0.88)), neck: wool, head: () => face, leg: () => [wool, face, shade(face, 0.6)] }), h = q.head;
        const cap = axis(h.p(0.09, 0, 0.11), 0);
        s.skin(cap.rings([[-0.09, 0.06, 0.04, 0.04], [0, 0.13, 0.08, 0.05], [0.08, 0.07, 0.04, 0.04]]), 8, wool);
        ears(s, h, [0.1, 0.1, 0.04], [0.08, 0.31, -0.03], 0.11, face);
        eyes(s, h, [0.2, 0.13, 0.04], 0.04);
        s.chain([[0, -0.54, 0.82], [0, -0.64, 0.7], [0, -0.66, 0.6]].map(q.body), [0.1, 0.1, 0.04], wool, 5);
    },
} satisfies Animal<number[]>;
