import { rgb } from '../../structures/mesh';
import type { Animal } from '../animals';
import { shade } from '../flat';
import { antlers, DEER } from './body/deer';
import { ears, eyes, HOOF, NOSE, quadruped, X } from './body/quadruped';

const RUMP = rgb('#e2d2b0'), ANTLER = rgb('#a89274');

interface Coat { coat: string; stag?: boolean }

export default {
    name: 'Red deer',
    latin: 'Cervus elaphus',
    height: 1.2,
    // A stag among two hinds.
    coats: [{ coat: '#96582e', stag: true }, { coat: '#a2673a' }, { coat: '#8c5a34' }] as Coat[],
    poses: 3,
    build(s, c, pose) {
        const coat = rgb(c.coat), dark = shade(coat, 0.7);
        const q = quadruped(s, DEER, pose, {
            trunk: (band, top) => (band < 1 ? RUMP : top < -0.6 ? shade(coat, 1.25) : coat), neck: c.stag ? dark : coat,
            head: d => (d > 1 ? NOSE : d > 0.75 ? shade(coat, 1.3) : coat), leg: () => [coat, shade(coat, 0.85), HOOF],
        }), h = q.head;
        ears(s, h, [0.08, 0.1, 0.04], [0.0, 0.32, 0.14], 0.13, coat);
        eyes(s, h, [0.19, 0.125, 0.045], 0.045);
        if (c.stag) {
            antlers(s, q.crown(0.06, 0.1), [[0, 0, 0.06], [0.12, 0.22, 0.19], [0.26, 0.44, 0.3], [0.26, 0.68, 0.3]], [[0, -0.2, 0.1, 0.1], [1, -0.1, 0.38, 0.28], [2, 0.42, 0.6, 0.36], [2, 0.12, 0.64, 0.2]], ANTLER);
            // The mane of the neck.
            s.fine(() => s.leaf(q.neck(0.1, -0.15), q.neck(0.8, -0.17), 0.16, 0.14, X, dark));
        }
        s.leaf(q.body([0, -0.5, 0.9]), q.body([0, -0.6, 0.78]), 0.09, 0.05, X, RUMP);
    },
} satisfies Animal<Coat>;
