// The mast of a railway's contact line, in metres, z up from the rail's top with the track's middle at the
// origin: the mast stands `reach` metres toward −y, and its cantilever carries the two wires over the origin.
import { rgb } from '../structures/mesh';
import { footing, Model, type Vec } from './flat';

/** The contact wire's height over the rail, and the messenger wire's where a mast carries it. */
export const WIRE_M = 5.5, MESSENGER_M = 6.9;

const STEEL = rgb('#8b918d'), WEB = rgb('#727a7b'), TUBE = rgb('#a1a7a2'), FITTING = rgb('#3d4346'), PORCELAIN = rgb('#8d4c38'), CONCRETE = rgb('#b5b0a4');
/** The mast's top, the heights where its two tubes leave it, and the height of the tube that steadies the contact wire. */
const TOP = 7.5, UPPER = 6.82, LOWER = 5.35, STEADY = 5.95;

/** A round tube of diameter d, open at its ends: each end lies in another part, or is too thin to show. */
const tube = (m: Model, a: Vec, b: Vec, d: number, colour: number, sides = 6) => m.beam(a, b, d / Math.SQRT2, colour, { sides });

/** An insulator near the start of a tube from `a` to `b`: three bells, each wide toward `a`. */
function insulator(m: Model, a: Vec, b: Vec): void {
    const l = Math.hypot(b[0] - a[0], b[1] - a[1], b[2] - a[2]), at = (k: number): Vec => [a[0] + (b[0] - a[0]) * k / l, a[1] + (b[1] - a[1]) * k / l, a[2] + (b[2] - a[2]) * k / l];
    for (const k of [0.1, 0.2, 0.3]) m.beam(at(k), at(k + 0.09), 0.23 / Math.SQRT2, PORCELAIN, { sides: 5, w1: 0.11 / Math.SQRT2 });
}

/**
 * A mast `reach` metres beside its track. On a deck it hangs from the deck's edge, else it stands in a concrete foot
 * that reaches below a bank.
 */
export function railMast(reach: number, deck: boolean): Model {
    const m = new Model(), y = -reach, face = y + 0.13;
    const bottom = deck ? -1 : -0.3, height = TOP - bottom, middle = bottom + height / 2;
    // An H-beam: its two flanges along the track, its web toward it.
    for (const side of [-1, 1]) m.box([0, y + side * 0.11, middle], [0.26, 0.04, height], STEEL);
    m.box([0, y, middle], [0.045, 0.18, height], WEB);
    if (deck) m.box([0, y + 0.2, -0.85], [0.36, 0.52, 0.12], FITTING);
    else {
        const deep = footing(0.4);
        m.box([0, y, -0.2 - deep / 2], [0.64, 0.64, deep], CONCRETE).box([0, y, -0.14], [0.5, 0.5, 0.12], CONCRETE);
    }
    // The cantilever: a level tube and a strut that rises to its outer end, where the messenger wire lies in a saddle.
    const top: Vec = [0, face, UPPER], foot: Vec = [0, face, LOWER], tip: Vec = [0, 0.3, UPPER], join: Vec = [0, -0.25, UPPER - 0.02];
    tube(m, top, tip, 0.09, TUBE);
    tube(m, foot, join, 0.1, TUBE);
    // The steady tube leaves the strut level; its arm comes back down to the contact wire.
    const share = (STEADY - LOWER) / (join[2] - LOWER), out = 0.62;
    tube(m, [0, face + share * (join[1] - face), STEADY], [0, out, STEADY], 0.075, TUBE);
    tube(m, [0, out - 0.06, STEADY - 0.02], [0, 0.02, WIRE_M + 0.07], 0.045, FITTING, 4);
    tube(m, [0, 0.3, UPPER], [0, 0.3, STEADY], 0.03, FITTING, 4);
    for (const [a, b] of [[top, tip], [foot, join]]) {
        m.box([0, face - 0.02, a[2]], [0.18, 0.1, 0.2], FITTING);
        insulator(m, a, b);
    }
    // The saddle's top is the messenger wire's height, the clamp's middle the contact wire's.
    return m.box([0, 0, MESSENGER_M - 0.03], [0.14, 0.12, 0.06], FITTING).box([0, 0, WIRE_M + 0.01], [0.16, 0.07, 0.1], FITTING);
}
