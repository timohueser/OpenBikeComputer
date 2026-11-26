// The body and the antlers that the deer share.
import { both, type Sculpt } from '../../sculpt';
import type { Body, quadruped } from './quadruped';

export const DEER: Body = {
    trunk: [[-0.54, 0.8, 0.1, 0.11, 0.11], [-0.46, 0.79, 0.21, 0.2, 0.2], [-0.2, 0.78, 0.25, 0.22, 0.23], [0.1, 0.77, 0.25, 0.22, 0.24], [0.32, 0.78, 0.22, 0.22, 0.23], [0.45, 0.8, 0.16, 0.17, 0.18], [0.52, 0.81, 0.08, 0.09, 0.1]],
    neck: { at: [0.34, 0.85], length: 0.48, size: [[0.16, 0.18, 0.2], [0.11, 0.125, 0.13], [0.095, 0.105, 0.105]], carry: [[66, 56], [-30, -55], [68, 58]] },
    head: { rows: [[0, 0.075, 0.08, 0.08], [0.06, 0.125, 0.115, 0.115], [0.18, 0.13, 0.115, 0.12], [0.32, 0.09, 0.08, 0.09], [0.42, 0.08, 0.065, 0.075], [0.46, 0.055, 0.045, 0.05]], nod: [-22, -72, -18], set: [0.05, 0.08] },
    front: { x: 0.13, y: 0.3, top: 0.68, thigh: 0.08, bone: 0.05 },
    hind: { x: 0.135, y: -0.38, top: 0.68, thigh: 0.095, bone: 0.05 },
};

/**
 * Both antlers from the crown frame: the beam through rows of [back, up, out], and tines as [the row of the beam they
 * leave, back, up, out] of their tips. Tier 1 keeps the beam and the first tine.
 */
export function antlers(s: Sculpt, k: ReturnType<ReturnType<typeof quadruped>['crown']>, beam: number[][], tines: number[][], colour: number, w = 0.055): void {
    both(side => {
        const at = ([back, up, out]: number[]) => k.p(-back, side * out, up), width = (i: number) => w * (1 - (0.8 * i) / (beam.length - 1));
        s.chain(beam.map(at), beam.map((_, i) => width(i)), colour, 3);
        tines.forEach(([from, ...tip], i) => (i && s.tier) || s.chain([at(beam[from]), at(tip)], [0.85 * width(from), 0.012], colour, 3));
    });
}
