// The body that the cattle breeds share.
import { shade } from '../../flat';
import { alongY, bellied, both, type Paint, type Sculpt } from '../../sculpt';
import { CREAM, ears, eyes, HOOF, HORN, NOSE, PINK, quadruped, type Body, type Coat } from './quadruped';

const BODY: Body = {
    trunk: [[-0.66, 0.74, 0.12, 0.14, 0.14], [-0.56, 0.72, 0.25, 0.25, 0.24], [-0.3, 0.72, 0.31, 0.28, 0.31], [0.05, 0.71, 0.32, 0.28, 0.33], [0.35, 0.73, 0.29, 0.27, 0.3], [0.52, 0.75, 0.22, 0.22, 0.24], [0.6, 0.76, 0.12, 0.13, 0.14]],
    neck: { at: [0.38, 0.78], length: 0.4, size: [[0.2, 0.2, 0.22], [0.135, 0.14, 0.15]], carry: [[24, 20], [-30, -60], [35, 25]] },
    head: { rows: [[0, 0.1, 0.1, 0.1], [0.07, 0.165, 0.15, 0.15], [0.2, 0.17, 0.15, 0.16], [0.34, 0.13, 0.11, 0.13], [0.44, 0.125, 0.1, 0.12], [0.5, 0.09, 0.07, 0.08]], nod: [-30, -72, -25] },
    front: { x: 0.17, y: 0.36, top: 0.62, thigh: 0.1, bone: 0.075 },
    hind: { x: 0.18, y: -0.42, top: 0.62, thigh: 0.115, bone: 0.075 },
};

export interface Cattle { coat: number; trunk?: Paint; neck?: number; head?: Coat['head']; shin?: number; ear?: number; tuft?: number; horns?: boolean }

/** A cow of the plain build: a breed gives its colours, and the paints of a patched one. */
export function cattle(s: Sculpt, pose: number, c: Cattle): void {
    const q = quadruped(s, BODY, pose, {
        trunk: c.trunk ?? bellied(c.coat, shade(c.coat, 0.85)), neck: c.neck ?? c.coat,
        head: c.head ?? (d => (d > 1 ? NOSE : d > 0.7 ? CREAM : c.coat)),
        leg: () => [c.coat, c.shin ?? c.coat, HOOF],
    }), h = q.head;
    ears(s, h, [0.1, 0.12, 0.03], [0.0, 0.38, -0.07], 0.19, c.ear ?? c.coat);
    if (c.horns ?? true) both(side => s.chain([h.p(0.06, side * 0.1, 0.12), h.p(0.05, side * 0.2, 0.2), h.p(0.07, side * 0.22, 0.3)], [0.06, 0.045, 0.015], HORN));
    eyes(s, h, [0.21, 0.165, 0.05]);
    s.fine(() => s.chain([[0, -0.66, 0.9], [0, -0.75, 0.6], [0, -0.75, 0.42]].map(q.body), [0.04, 0.03, 0.03], c.coat));
    s.fine(() => s.chain([q.body([0, -0.75, 0.44]), q.body([0, -0.75, 0.28])], [0.08, 0.03], c.tuft ?? NOSE));
    if (!q.lies) s.fine(() => s.skin(alongY([[-0.25, 0.46, 0.11, 0.13, 0.13, -90], [-0.25, 0.36, 0.09, 0.11, 0.11, -90], [-0.25, 0.33, 0.05, 0.06, 0.06, -90]]), 6, PINK));
}
