import { rgb } from '../../structures/mesh';
import type { Animal } from '../animals';
import { shade } from '../flat';
import { alongY, axis, bands, bellied, both } from '../sculpt';

const EYE = rgb('#1c1614'), HOOF = rgb('#2c2420'), HORN = rgb('#e2d6b8'), PINK = rgb('#d8a79a'), CREAM = rgb('#e4d8c2'), NOSE = rgb('#3a2e2a');

export default {
    name: 'Brown cattle',
    latin: 'Bos taurus',
    height: 1.4,
    coats: ['#9a7a5c', '#8e4a24', '#ae8250'].map(rgb),
    poses: 1,
    build(s, coat) {
        const h = axis([0, 0.66, 1.0], -30);
        s.skin(alongY([[-0.66, 0.74, 0.12, 0.14, 0.14], [-0.56, 0.72, 0.25, 0.25, 0.24], [-0.3, 0.72, 0.31, 0.28, 0.31], [0.05, 0.71, 0.32, 0.28, 0.33], [0.35, 0.73, 0.29, 0.27, 0.3], [0.52, 0.75, 0.22, 0.22, 0.24], [0.6, 0.76, 0.12, 0.13, 0.14]]), 10, bellied(coat, shade(coat, 0.85)));
        s.skin(alongY([[0.38, 0.78, 0.2, 0.2, 0.22, 30], [0.74, 0.93, 0.135, 0.14, 0.15, 25]]), 8, coat);
        s.skin(h.rings([[0, 0.1, 0.1, 0.1], [0.07, 0.165, 0.15, 0.15], [0.2, 0.17, 0.15, 0.16], [0.34, 0.13, 0.11, 0.13], [0.44, 0.125, 0.1, 0.12], [0.5, 0.09, 0.07, 0.08]]), 8, bands(coat, coat, coat, coat, CREAM, CREAM, NOSE));
        const leg = (y: number, k: number) => [[y, 0.62, 0.1 * k, 0.12 * k, 0.12 * k], [y, 0.36, 0.075, 0.085, 0.085], [y, 0.1, 0.065, 0.07, 0.07], [y, 0.085, 0.08, 0.09, 0.085], [y, 0, 0.085, 0.1, 0.09]];
        for (const [x, y, k] of [[0.17, 0.36, 1], [0.18, -0.42, 1.15]]) both(side => s.skin(leg(y, k).map(([ly, z, w, up, down]) => ({ at: [side * x, ly, z], w, up, down, pitch: -90, e: 2 })), 6, bands(coat, coat, coat, HOOF)));
        both(side => {
            s.leaf(h.p(0.1, side * 0.13, 0.05), h.p(0.08, side * 0.4, 0), 0.14, 0.05, h.dir, coat);
            s.chain([h.p(0.06, side * 0.1, 0.12), h.p(0.05, side * 0.2, 0.2), h.p(0.07, side * 0.22, 0.3)], [0.06, 0.045, 0.015], HORN);
            s.fine(() => s.box(h.p(0.21, side * 0.165, 0.05), [0.03, 0.05, 0.05], EYE));
        });
        s.fine(() => s.chain([[0, -0.66, 0.9], [0, -0.75, 0.6], [0, -0.75, 0.42]], [0.04, 0.03, 0.03], coat));
        s.fine(() => s.chain([[0, -0.75, 0.44], [0, -0.75, 0.28]], [0.08, 0.03], NOSE));
        s.fine(() => s.skin(alongY([[-0.25, 0.46, 0.11, 0.13, 0.13, -90], [-0.25, 0.36, 0.09, 0.11, 0.11, -90], [-0.25, 0.33, 0.05, 0.06, 0.06, -90]]), 6, PINK));
    },
} satisfies Animal<number>;
