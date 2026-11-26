// The body that every herd animal shares: a trunk, a bent neck, a head and four legs from the numbers of a breed, in a
// pose. The breed adds what makes it itself (ears, horns, a tail) at the frames this returns.
import { rgb } from '../../../structures/mesh';
import type { Vec } from '../../flat';
import { alongY, axis, both, through, type Paint, type Sculpt } from '../../sculpt';

/** The poses: it stands with its head up, it grazes, it lies with its legs folded. */
export const STAND = 0, GRAZE = 1, LIE = 2;
export const EYE = rgb('#1c1614'), HOOF = rgb('#2c2420'), NOSE = rgb('#3a2e2a'), HORN = rgb('#e2d6b8'), PINK = rgb('#d8a79a'), CREAM = rgb('#e4d8c2');
export const X: Vec = [1, 0, 0];

/** A pair of legs: x and y of the feet, the top inside the trunk, the half-width of the thigh and of the bone. */
export interface Leg { x: number; y: number; top: number; thigh: number; bone: number }

export interface Body {
    /** Rows of [y, z, half-width, up, down] from the rump to the chest. */
    trunk: number[][];
    sides?: number;
    wool?: number;
    neck: {
        /** [y, z] of its root in the chest. */
        at: number[];
        length: number;
        /** Rows of [half-width, up, down] from the root to the head, spread evenly. */
        size: number[][];
        /** For each pose, the pitch of the neck at its root and at the head. */
        carry: number[][];
    };
    head: {
        /** Rows of [d, half-width, up, down] from the poll to the nose. */
        rows: number[][];
        /** The pitch of the head for each pose. */
        nod: number[];
        /** How far the poll sits behind and above the end of the neck. */
        set?: number[];
    };
    front: Leg;
    hind: Leg;
}

export interface Coat {
    trunk: Paint | number;
    neck: Paint | number;
    /** From how far along the head a band ends (0 to 1; the front cap is 2) and how far up its facet looks. */
    head: (d: number, top: number) => number;
    /** The thigh, the shin and the hoof; the front pair when `front`. */
    leg: (front: boolean) => number[];
}

/** How deep a lying trunk sinks into the grass. */
const SINK = 0.03;
/** The longest leg that folds at its full length: a longer one would stand out over the lying trunk. */
const LONGEST_FOLD = 0.64;
const mix = (rows: number[][], t: number) => {
    const at = t * (rows.length - 1), i = Math.min(rows.length - 2, Math.floor(at));
    return rows[i].map((v, k) => v + (rows[i + 1][k] - v) * (at - i));
};

/**
 * Builds the body and returns its frames: `head` (an axis from the poll), `crown(d, v)` (an axis from there on the head
 * that is level when the animal stands: horns hang from it), `neck(t, v)` (the point v above the neck at its share t),
 * `body` (a point of the standing trunk in this pose), `lies`.
 */
export function quadruped(s: Sculpt, b: Body, pose: number, coat: Coat) {
    const lies = pose === LIE, far = s.tier > 0;
    const drop = lies ? Math.min(...b.trunk.slice(1, -1).map(([, z, , , down]) => z - down)) + SINK : 0;
    const body = ([x, y, z]: number[]): Vec => [x, y, z - drop];
    s.skin(alongY(b.trunk.map(([y, z, ...size]) => [y, z - drop, ...size])), b.sides ?? 10, coat.trunk, b.wool);

    const n = far ? 2 : 3, [from, to] = b.neck.carry[pose], bones: Vec[] = [[0, b.neck.at[0], b.neck.at[1] - drop]];
    for (let i = 0; i < n; i++) {
        const a = ((from + ((to - from) * (i + 0.5)) / n) * Math.PI) / 180, [, y, z] = bones[i];
        bones.push([0, y + (b.neck.length / n) * Math.cos(a), z + (b.neck.length / n) * Math.sin(a)]);
    }
    const rings = through(bones, bones.map((_, i) => mix(b.neck.size, i / n)));
    s.skin(rings, 8, coat.neck);
    const neck = (t: number, v = 0, x = 0): Vec => {
        const i = Math.min(n - 1, Math.floor(t * n)), k = t * n - i, p = (rings[i].pitch + (rings[i + 1].pitch - rings[i].pitch) * k) * (Math.PI / 180);
        return [x, bones[i][1] + (bones[i + 1][1] - bones[i][1]) * k - Math.sin(p) * v, bones[i][2] + (bones[i + 1][2] - bones[i][2]) * k + Math.cos(p) * v];
    };

    const nod = b.head.nod[pose], [back, up] = b.head.set ?? [0.1, 0.02], head = axis(axis(bones[n], nod).p(-back, 0, up), nod);
    const rows = b.head.rows.filter((_, i, all) => !far || i % 2 === 0 || i === all.length - 1), length = rows[rows.length - 1][0];
    s.skin(head.rings(rows), 8, (band, top) => coat.head(band < 0 ? 0 : band >= rows.length - 1 ? 2 : rows[band + 1][0] / length, top));

    for (const front of [true, false]) {
        const leg = front ? b.front : b.hind, { x, y, thigh, bone } = leg, top = lies ? Math.min(leg.top, LONGEST_FOLD) : leg.top, [upper, lower, hoof] = coat.leg(front), step = 1.13 * bone;
        both(side => {
            const at = (dx: number, py: number, z: number): Vec => [side * (x + dx), py, z];
            if (!lies) {
                const hock = front ? 0 : -0.45 * bone;
                const down = [[y, top, thigh, 1.2 * thigh, 1.2 * thigh], [y + hock, 0.56 * top, bone, 1.13 * bone, 1.13 * bone], [y + 0.3 * hock, step + 0.015, 0.87 * bone, 0.93 * bone, 0.93 * bone],
                    [y + 0.3 * hock, step, 1.07 * bone, 1.2 * bone, 1.13 * bone], [y + 0.3 * hock, 0, 1.13 * bone, 1.33 * bone, 1.2 * bone]].filter((_, i) => !far || i !== 2 && i !== 3);
                s.skin(down.map(([py, z, w, fore, aft]) => ({ at: at(0, py, z), w, up: fore, down: aft, pitch: -90, e: 2 })), 6, far ? (band => [upper, upper, lower, lower][band + 1]) : (band => [upper, upper, lower, lower, hoof, hoof][band + 1]));
            } else if (front) {
                // The folded foreleg: the knee pokes out before the chest, the hoof lies by the elbow.
                const fold = [[y - 0.1 * top, 1.6, 1.3, 1.5, 1.6], [y + 0.42 * top, 1.4, 1.2, 1.3, 1.4], [y + 0.5 * top, 1.2, 0.7, 0.8, 0.8]].filter((_, i) => !far || i !== 1);
                s.skin(through(fold.map(([py, z]) => at(0, py, z * bone)), fold.map(row => row.slice(2).map(v => v * bone))), 6, lower);
                s.fine(() => s.skin(through([at(1.2 * bone, y + 0.02 * top, 0.9 * bone), at(1.2 * bone, y + 0.2 * top, 0.9 * bone)], [[bone, bone, 0.9 * bone], [1.1 * bone, 1.1 * bone, 0.9 * bone]]), 6, hoof));
            } else {
                // The haunch on the flank, and the shank along the grass to a hoof beside the belly.
                s.skin(through([at(thigh, y - 0.3 * top, 0.3 * top), at(1.1 * thigh, y + 0.02 * top, 0.33 * top), at(thigh, y + 0.36 * top, 0.24 * top)], [[0.6 * thigh, 0.14 * top, 0.14 * top], [1.1 * thigh, 0.33 * top, 0.33 * top], [0.6 * thigh, 0.16 * top, 0.16 * top]]), 6, upper);
                s.fine(() => s.skin(through([at(1.6 * thigh, y - 0.26 * top, 1.2 * bone), at(1.8 * thigh, y + 0.45 * top, 1.1 * bone), at(1.8 * thigh, y + 0.62 * top, 1.1 * bone)], [[1.1 * bone, 1.2 * bone, 1.2 * bone], [0.95 * bone, bone, bone], [1.1 * bone, 1.1 * bone, 1.1 * bone]]), 6, band => (band > 0 ? hoof : lower)));
            }
        });
    }
    return { head, neck, body, lies, crown: (d: number, v: number) => axis(head.p(d, 0, v), nod - b.head.nod[STAND]) };
}

/**
 * A paint from a map of the pelt: one word for each band, its letters the facets from low on the right flank over the
 * back to low on the left flank. The belly, and a letter with no colour, take `base`.
 */
export const pelt = (map: string[], colours: Record<string, number>, base: number): Paint => (band, _, around) =>
    colours[map[Math.max(0, Math.min(map.length - 1, band))][Math.round(around * 10 + 1) % 10]] ?? base;

type Head = ReturnType<typeof axis>;
/** Both eyes at [d, x, v] of the head: fine details. */
export const eyes = (s: Sculpt, h: Head, [d, x, v]: number[], size = 0.05) => s.fine(() => both(side => s.box(h.p(d, side * x, v), [0.6 * size, size, size], EYE)));
/** Both ears, leaves from [d, x, v] of the head to their tips. */
export const ears = (s: Sculpt, h: Head, [d, x, v]: number[], [td, tx, tv]: number[], width: number, colour: number) =>
    both(side => s.leaf(h.p(d, side * x, v), h.p(td, side * tx, tv), width, 0.6 * width, h.dir, colour));
