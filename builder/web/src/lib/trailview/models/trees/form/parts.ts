// The levels of detail of a tree and the crown parts that every form shares: limbs, twigs, berries and flower heads.

import { computeNormals, hex, lathe, limb, lump, paint, place, revolve, smooth, soft, tube, turnFromTo, UP, v3, type Colour, type Geometry, type Sheet, type V } from '../../shape';
import type { Broadleaf, Lod } from '../../trees';

export const lerp = (a: number, b: number, t: number) => a + (b - a) * t;
export const clamp = (x: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, x));

/**
 * Segments around a part, spline rows per tier, conifer tiers and broadleaf crown lumps. A trunk
 * has its own sides and rows per profile span. A bare crown's limbs have `limbs` sides, or are flat
 * at 0, and it keeps every `thin`-th twig of the nearest level.
 */
export const DETAIL = [
    { sides: 22, rows: 10, tiers: 7, trunk: [10, 2], lumps: 9, ico: 3, limbs: 6, thin: 1 },
    { sides: 10, rows: 6, tiers: 5, trunk: [6, 1], lumps: 6, ico: 1, limbs: 4, thin: 3 },
    { sides: 6, rows: 3, tiers: 3, trunk: [4, 1], lumps: 5, ico: 0, limbs: 0, thin: 6 },
] as const;

/**
 * A bare crown has TWIGS twigs at the nearest level, each TWIG_WIDTH wide at its root in tree
 * heights, a conifer's a thirtieth as many per branch. Every level grows the limbs, whorls and twigs
 * of the nearest; a farther level keeps every `thin`-th twig, `thin` times as wide, so the crown keeps
 * its outline and covers as much.
 */
export const TWIGS = 400, TWIG_WIDTH = 0.004;

/** The lowest point of some parts. */
export function lowest(parts: Geometry[]): number {
    let low = Infinity;
    for (const { position } of parts) for (let k = 2; k < position.length; k += 3) low = Math.min(low, position[k]);
    return low;
}

export const vertices = (parts: Geometry[]) => parts.reduce((sum, part) => sum + part.position.length / 3, 0);

/** A berry's radius, in tree heights. */
const BEAD = 0.009;

/**
 * Random corners on the skin of a crown's `parts`, while `more` holds and for at most `tries` draws: those whose normal
 * `faces` allows and that `hidden` does not rule out, as corners inside another part. Each with its place, its normal
 * and its draw.
 */
function* skin(parts: Geometry[], rand: () => number, tries: number, more: () => boolean, faces: (n: V) => boolean, hidden: (p: V, part: number) => boolean): Generator<[V, V, number]> {
    const p = v3(), n = v3();
    for (let t = 0; t < tries && more(); t++) {
        const part = Math.floor(rand() * parts.length), k = Math.floor((rand() * parts[part].position.length) / 3);
        p.from(parts[part].position, k);
        if (!faces(n.from(parts[part].normal, k)) || hidden(p, part)) continue;
        yield [p, n, t];
    }
}

/** Berry clusters on the side of a crown's `parts` (`skin`), three beads each. */
export function berries(parts: Geometry[], [colour, count]: [string, number], rand: () => number, hidden: (p: V, part: number) => boolean): Geometry[] {
    const out: Geometry[] = [], shade = hex(colour);
    for (const [p, n] of skin(parts, rand, 1000, () => out.length < 3 * count, m => Math.abs(m.z) <= 0.6, hidden)) {
        for (let b = 0; b < 3; b++) {
            const a = 2 * Math.PI * (b / 3 + rand()), at = p.clone().addScaledVector(n, 0.5 * BEAD).add(v3(0.9 * BEAD * Math.cos(a), 0.9 * BEAD * Math.sin(a), BEAD * (rand() - 0.5)));
            out.push(place(paint(soft(lump(BEAD, 0)), (_, m) => shade.clone().multiplyScalar(0.8 + 0.4 * smooth(-0.5, 1, m.z))), { at }));
        }
    }
    return out;
}

// A flower head, radius 1 and 1 tall, over a collar that stands in the crown.
const HEAD: [number, number][] = [[0.9, -0.3], [1, 0.1], [0.8, 0.6], [0, 1]];

/**
 * Heads on the skin of a crown's `parts` (`skin`), not on its underside: each faces out of the crown and up, in the
 * fleck's colour over a collar of the leaves' colour `leaf`.
 */
export function heads(parts: Geometry[], { count, width, height, points }: NonNullable<Broadleaf['heads']>, [colour, share]: [string, number], leaf: Colour, rand: () => number, hidden: (p: V, part: number) => boolean): Geometry[] {
    const out: Geometry[] = [], shade = hex(colour);
    for (const [p, n, tries] of skin(parts, rand, 2000, () => out.length < Math.round(count * share), m => m.z >= -0.3, hidden)) {
        // A star is flat: a point in the middle and a rim.
        const geo = points ? soft(revolve([[1, 0], [0, 0.4]], 2 * points)) : soft(lathe(HEAD, 6, 2), 0.1, 5, tries), q = geo.position;
        // A star's points stand out and hang a little.
        for (let i = 0; points && i < q.length; i += 3) {
            const far = 0.5 + 0.5 * Math.cos(points * Math.atan2(q[i + 1], q[i])), k = lerp(0.3, 1.4, far);
            q.splice(i, 3, q[i] * k, q[i + 1] * k, q[i + 2] - 0.4 * far * Math.hypot(q[i], q[i + 1]));
        }
        const tone = 0.85 + 0.3 * rand(), size = 0.5 * width * (0.8 + 0.4 * rand());
        const head = paint(computeNormals(geo), (at, m) => leaf.clone().lerp(shade.clone().multiplyScalar(tone * (0.85 + 0.2 * m.z)), smooth(-0.15, 0.1, at.z)));
        out.push(place(head, { at: p.clone(), turn: turnFromTo(UP, n.lerp(UP, 0.5).normalize()), scale: [size, size, 2 * size * height] }));
    }
    return out;
}

/**
 * Bends a part made along z from 0 to 1 onto `path` from `from` to `to`. `across` gives where a point (x, y)
 * at z stands beside the path: level, and up.
 */
export function bend(geo: Geometry, path: (s: number) => V, from: number, to: number, across: (x: number, y: number, z: number) => [number, number]): Geometry {
    const p = geo.position;
    for (let i = 0; i < p.length; i += 3) {
        const s = lerp(from, to, p[i + 2]), along = path(s + 0.01).sub(path(s - 0.01)).normalize();
        const side = v3().crossVectors(UP, along);
        if (side.lengthSq() < 1e-6) side.set(1, 0, 0);
        const lift = along.cross(side.normalize()), [x, y] = across(p[i], p[i + 1], p[i + 2]);
        const at = path(s).addScaledVector(side, x).addScaledVector(lift, y);
        p.splice(i, 3, at.x, at.y, at.z);
    }
    return computeNormals(geo);
}

/**
 * Leans the normals of a thin part by `share` towards the way out of the tree and up, so that many thin parts
 * light as one round crown and not each as a slab.
 */
export function rounded(geo: Geometry, share: number): Geometry {
    const n = v3(), p = v3();
    for (let k = 0; k < geo.normal.length / 3; k++) {
        n.from(geo.normal, k).lerp(p.from(geo.position, k).setZ(0).normalize().lerp(UP, 0.4).normalize(), share).normalize();
        geo.normal.splice(3 * k, 3, n.x, n.y, n.z);
    }
    return geo;
}

/**
 * Bare limb `k`, `radius` at its root and `tip` at its end: round with `sides` sides, or at 0 sides one
 * upright triangle among the twigs, which faces by k and draws no random number, so every level draws
 * the same twigs.
 */
export function bough(parts: Geometry[], twigs: Sheet, from: V, to: V, radius: number, bark: (p: V) => Colour, sides: number, k: number, tip = radius): void {
    if (sides) parts.push(limb(from.toArray(), to.toArray(), radius, bark, 0, sides, tip));
    else upright(twigs, from, to, 2 * radius, [bark(from), bark(to)], k % 2 ? -1 : 1);
}

/** Bare stem `k` through `points`, as `bough`: a tube of `sides` sides, or at 0 sides a flat ribbon among the twigs. */
export function stem(parts: Geometry[], twigs: Sheet, points: V[], radius: number, tip: number, bark: (p: V) => Colour, sides: number, k: number): void {
    if (sides) return void parts.push(tube(points, radius, tip, sides, bark));
    const last = points.length - 1, along = v3().subVectors(points[last], points[0]);
    const across = v3(0, 0, 1).addScaledVector(along, -along.z / along.lengthSq());
    if (across.lengthSq() < 1e-6) across.set(1, 0, 0);
    across.setLength(k % 2 ? -1 : 1);
    const edge = (i: number, sign: number) => points[i].clone().addScaledVector(across, sign * lerp(radius, i === last ? 0 : tip, i / last));
    for (let i = 0; i < last; i++) {
        const [a, b] = [bark(points[i]), bark(points[i + 1])];
        twigs.add([edge(i, -1), edge(i, 1), edge(i + 1, 1)], [a, a, b]);
        if (i + 1 < last) twigs.add([edge(i, -1), edge(i + 1, 1), edge(i + 1, -1)], [a, b, b]);
    }
}

/** Twig `k` of a bare crown at a level: every `thin`-th twig stands, `thin` times as wide (DETAIL, TWIGS). */
export function twig(twigs: Sheet, root: V, tip: V, colours: Colour[], rand: () => number, lod: Lod, k: number): void {
    const { thin } = DETAIL[lod], face = rand() < 0.5 ? 1 : -1;
    if (k % thin === 0) upright(twigs, root, tip, thin * TWIG_WIDTH, colours, face);
}

/**
 * One upright triangle from `width` across at its root to a point at its tip, facing `face` (±1).
 * Upright, it shows from the side and its normal stays level; twigs face either way, so half of them
 * catch the sun wherever they are in the crown.
 */
export function upright(twigs: Sheet, root: V, tip: V, width: number, [base, end]: Colour[], face: number): void {
    const along = v3().subVectors(tip, root);
    const across = v3(0, 0, 1).addScaledVector(along, -along.z / along.lengthSq());
    if (across.lengthSq() < 1e-6) across.set(1, 0, 0);
    across.setLength(0.5 * face * width);
    twigs.add([root.clone().sub(across), root.clone().add(across), tip], [base, base, end]);
}

/** Draws the underside of a lump of radius r down into a hanging point. */
export function hang(geo: Geometry, r: number, amount: number): Geometry {
    const p = geo.position;
    for (let i = 0; i < p.length; i += 3) {
        const k = smooth(0.2, -1, p[i + 2] / r);
        const narrow = 1 - 0.5 * amount * k;
        p.splice(i, 3, p[i] * narrow, p[i + 1] * narrow, p[i + 2] - amount * r * k * k);
    }
    return computeNormals(geo);
}
