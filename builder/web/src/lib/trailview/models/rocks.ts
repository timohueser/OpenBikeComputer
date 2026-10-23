// Rocks 1 m tall with their base at the origin, z up: faceted lumps sunk into the ground, each face in its
// own colour, from the prototype's src/models/rocks.ts. A crag is a leaning blade taller than wide.
import { hex, Model, type Vec } from './shape';

/** Grey body, a lit top and lichen; the shader multiplies them by the rock under them over GREY. */
export const GREY = hex('#8e908a');
const BODY = ['#7d7f78', '#8e908a', '#a3a49c'].map(hex), LICHEN = hex('#b4b05a');

/** An icosahedron cut once: 80 faces on the unit sphere. */
function lump(): Vec[][] {
    const t = (1 + Math.sqrt(5)) / 2, unit = (p: Vec): Vec => {
        const l = Math.hypot(...p);
        return [p[0] / l, p[1] / l, p[2] / l];
    };
    const v: Vec[] = [[-1, t, 0], [1, t, 0], [-1, -t, 0], [1, -t, 0], [0, -1, t], [0, 1, t], [0, -1, -t], [0, 1, -t], [t, 0, -1], [t, 0, 1], [-t, 0, -1], [-t, 0, 1]].map(p => unit(p as Vec));
    const faces = [[0, 11, 5], [0, 5, 1], [0, 1, 7], [0, 7, 10], [0, 10, 11], [1, 5, 9], [5, 11, 4], [11, 10, 2], [10, 7, 6], [7, 1, 8],
        [3, 9, 4], [3, 4, 2], [3, 2, 6], [3, 6, 8], [3, 8, 9], [4, 9, 5], [2, 4, 11], [6, 2, 10], [8, 6, 7], [9, 8, 1]];
    const mid = (a: Vec, b: Vec) => unit([(a[0] + b[0]) / 2, (a[1] + b[1]) / 2, (a[2] + b[2]) / 2]);
    return faces.flatMap(([i, j, k]) => {
        const [a, b, c] = [v[i], v[j], v[k]], [ab, bc, ca] = [mid(a, b), mid(b, c), mid(c, a)];
        return [[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]];
    });
}

const smooth = (a: number, b: number, x: number) => {
    const t = Math.min(1, Math.max(0, (x - a) / (b - a)));
    return t * t * (3 - 2 * t);
};
const wobble = ([x, y, z]: Vec, s: number) => Math.sin(x * 2.2 + s) * Math.sin(y * 2.9 + 2.1 * s) * Math.sin(z * 1.9 + 0.7 * s) + 0.5 * Math.sin(x * 4.7 - s) * Math.sin(z * 3.8 + 1.3 * s);
const fleck = ([x, y, z]: Vec, s: number) => {
    const h = Math.sin(x * 127.1 + y * 311.7 + z * 74.7 + s * 17.3) * 43758.5453;
    return h - Math.floor(h);
};

/** A crag: a lump stretched up and leant over, a fifth of it below the ground. */
export function crag(seed: number): Model {
    const shaped = lump().map(face => face.map(p => {
        const k = 1 + 0.22 * wobble(p, seed), q: Vec = [p[0] * k, p[1] * k, p[2] * k];
        const z = Math.max(q[2], -0.35) * 1.9;
        return [q[0] * 0.55 + 0.25 * z, q[1] * 0.9, z] as Vec;
    }));
    const zs = shaped.flat().map(p => p[2]), low = Math.min(...zs), high = Math.max(...zs), lift = -low - 0.2 * (high - low), scale = 1 / (high + lift);
    const m = new Model();
    for (const face of shaped) {
        const [a, b, c] = face.map(([x, y, z]): Vec => [x * scale, y * scale, (z + lift) * scale]);
        const u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]], w = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        const n = [u[1] * w[2] - u[2] * w[1], u[2] * w[0] - u[0] * w[2], u[0] * w[1] - u[1] * w[0]], nz = n[2] / (Math.hypot(...n) || 1);
        const lit = smooth(0.6, 0.95, nz), body = nz < 0 ? BODY[0] : BODY[1];
        const colour = fleck(a, seed) < 0.15 && nz > 0.3 ? LICHEN : lit > 0.5 ? BODY[2] : body;
        m.tri(a, b, c, colour);
    }
    return m;
}
