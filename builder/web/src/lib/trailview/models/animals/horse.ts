import { rgb } from '../../structures/mesh';
import type { Animal } from '../animals';
import { shade } from '../flat';
import { bellied } from '../sculpt';
import { ears, eyes, quadruped, type Body } from './body/quadruped';

const BODY: Body = {
    trunk: [[-0.58, 0.8, 0.11, 0.13, 0.13], [-0.5, 0.78, 0.24, 0.23, 0.23], [-0.28, 0.77, 0.28, 0.24, 0.25], [0.02, 0.75, 0.28, 0.23, 0.26], [0.28, 0.77, 0.26, 0.24, 0.26], [0.43, 0.79, 0.2, 0.21, 0.23], [0.51, 0.8, 0.11, 0.12, 0.14]],
    neck: { at: [0.33, 0.82], length: 0.58, size: [[0.18, 0.22, 0.26], [0.12, 0.16, 0.17], [0.095, 0.125, 0.115]], carry: [[52, 34], [-25, -50], [55, 40]] },
    head: { rows: [[0, 0.08, 0.09, 0.09], [0.07, 0.135, 0.13, 0.13], [0.22, 0.14, 0.13, 0.14], [0.4, 0.1, 0.09, 0.11], [0.53, 0.095, 0.08, 0.1], [0.58, 0.07, 0.055, 0.065]], nod: [-48, -70, -40], set: [0.08, 0.04] },
    front: { x: 0.14, y: 0.31, top: 0.7, thigh: 0.09, bone: 0.058 },
    hind: { x: 0.15, y: -0.41, top: 0.7, thigh: 0.105, bone: 0.058 },
};

interface Coat { coat: string; hair: string; shin: string; blaze?: boolean }

export default {
    name: 'Horse',
    latin: 'Equus caballus',
    height: 1.55,
    // A bay with a blaze, a chestnut, a grey, a black.
    coats: [{ coat: '#7a3e1e', hair: '#1a1512', shin: '#2e1e16', blaze: true }, { coat: '#8a4018', hair: '#6e3014', shin: '#8a4018' }, { coat: '#b8b6b2', hair: '#6e6c6a', shin: '#8e8c8a' }, { coat: '#4a4441', hair: '#1c1a19', shin: '#4a4441' }] as Coat[],
    poses: 3,
    build(s, c, pose) {
        const [coat, hair, shin] = [c.coat, c.hair, c.shin].map(hex => rgb(hex)), dark = shade(coat, 0.6);
        const q = quadruped(s, BODY, pose, { trunk: bellied(coat, shade(coat, 0.85)), neck: coat, head: (d, top) => (c.blaze && top > 0.9 && d > 0.3 ? rgb('#ece6da') : d > 0.85 ? dark : coat), leg: () => [coat, shin, rgb('#2a2624')] }), h = q.head;
        ears(s, h, [0.05, 0.07, 0.1], [0.02, 0.1, 0.27], 0.09, coat);
        eyes(s, h, [0.22, 0.135, 0.05]);
        // The mane on the crest, the forelock, the tail.
        s.chain([q.neck(0.12, 0.2), q.neck(0.5, 0.17), q.neck(0.95, 0.15), h.p(0.14, 0, 0.15)], [0.09, 0.11, 0.1, 0.06], hair);
        s.chain([[0, -0.56, 0.9], [0, -0.7, 0.74], [0, -0.74, 0.42], [0, -0.7, 0.22]].map(q.body), [0.07, 0.12, 0.13, 0.05], hair);
    },
} satisfies Animal<Coat>;
