import { MeshoptSimplifier } from 'meshoptimizer';
import type { Grown } from './trees';

/** The weights of a vertex's normal and colour against its place, for the simplifier. */
const NORMAL_WEIGHT = 0.5, COLOUR_WEIGHT = 1;

/** Resolves when the simplifier can run. */
export const simplifierReady = MeshoptSimplifier.ready;

/**
 * Tier k of a tree: the trunk of its own tier, which is a few triangles, and the crown of tier 0 with
 * at most `triangles` triangles, made by collapsing the edges where the place, the normal and the
 * colour change least. So every tier has the same crown parts, outline, normals and colours.
 */
export function simplified(near: Grown, own: Grown, triangles: number): Grown {
    const n = near.position.length / 3, crownIndex = crownTriangles(near);
    const attributes = new Float32Array(6 * n);
    for (let k = 0; k < n; k++) attributes.set([near.normal[3 * k], near.normal[3 * k + 1], near.normal[3 * k + 2], near.color[3 * k], near.color[3 * k + 1], near.color[3 * k + 2]], 6 * k);
    const weights = [NORMAL_WEIGHT, NORMAL_WEIGHT, NORMAL_WEIGHT, COLOUR_WEIGHT, COLOUR_WEIGHT, COLOUR_WEIGHT];
    const [index] = MeshoptSimplifier.simplifyWithAttributes(crownIndex, Float32Array.from(near.position), 3, attributes, 6, weights, null, 3 * triangles, 1);
    const kept = [...new Set(index)].sort((a, b) => a - b), at = new Map(kept.map((v, k) => [v, k + own.trunk]));
    const pick = (values: ArrayLike<number>) => kept.flatMap(v => [values[3 * v], values[3 * v + 1], values[3 * v + 2]]);
    const trunk = (values: ArrayLike<number>) => Array.from(values).slice(0, 3 * own.trunk);
    const ownIndex = Array.from(own.index), trunkIndex: number[] = [];
    for (let t = 0; t < ownIndex.length; t += 3) if (ownIndex[t] < own.trunk && ownIndex[t + 1] < own.trunk && ownIndex[t + 2] < own.trunk) trunkIndex.push(ownIndex[t], ownIndex[t + 1], ownIndex[t + 2]);
    const far: Grown = {
        position: [...trunk(own.position), ...pick(near.position)], normal: [...trunk(own.normal), ...pick(near.normal)], color: [...trunk(own.color), ...pick(near.color)],
        index: [...trunkIndex, ...Array.from(index, v => at.get(v)!)], trunk: own.trunk, crown: near.crown,
    };
    // Fewer triangles show the crown's lit and shaded sides in other shares: the crown's colours
    // scale so that it draws as bright as tier 0, seen from the side.
    const want = lit(near), got = lit(far);
    for (let k = 3 * far.trunk; k < far.color.length; k++) far.color[k] *= want[k % 3] / Math.max(got[k % 3], 1e-6);
    return far;
}

/** Texels a side of the views that `lit` draws, the sun's direction, and its weight against the sky. */
const VIEW = 96, SUN = [0.5, -0.5, 0.71], SUN_WEIGHT = 1;

/**
 * The mean colour of a tree 1 tall seen from four sides, under a sun 45 degrees up and the sky, as
 * draw.wgsl lights it in the main (light_up without its bands): drawn with a depth buffer, so the
 * inside of the crown counts only where it shows.
 */
export function lit({ position: p, normal: n, color: c, index }: Grown): number[] {
    const sum = [0, 0, 0];
    let count = 0;
    for (let view = 0; view < 4; view++) {
        const a = (view * Math.PI) / 2, cos = Math.cos(a), sin = Math.sin(a);
        const depth = new Float32Array(VIEW * VIEW).fill(-Infinity), shade = new Float32Array(3 * VIEW * VIEW);
        const x = (k: number) => (0.5 + cos * p[3 * k] - sin * p[3 * k + 1]) * VIEW, y = (k: number) => (1 - p[3 * k + 2]) * VIEW, z = (k: number) => sin * p[3 * k] + cos * p[3 * k + 1];
        for (let t = 0; t < index.length; t += 3) {
            const [i, j, k] = [index[t], index[t + 1], index[t + 2]];
            const area = (x(j) - x(i)) * (y(k) - y(i)) - (y(j) - y(i)) * (x(k) - x(i));
            if (area === 0) continue;
            for (let py = Math.max(0, Math.floor(Math.min(y(i), y(j), y(k)))); py <= Math.min(VIEW - 1, Math.max(y(i), y(j), y(k))); py++)
                for (let px = Math.max(0, Math.floor(Math.min(x(i), x(j), x(k)))); px <= Math.min(VIEW - 1, Math.max(x(i), x(j), x(k))); px++) {
                    const u = ((px + 0.5 - x(i)) * (y(k) - y(i)) - (py + 0.5 - y(i)) * (x(k) - x(i))) / area, v = ((x(j) - x(i)) * (py + 0.5 - y(i)) - (y(j) - y(i)) * (px + 0.5 - x(i))) / area;
                    if (u < 0 || v < 0 || u + v > 1) continue;
                    const w = [1 - u - v, u, v], o = py * VIEW + px, d = w[0] * z(i) + w[1] * z(j) + w[2] * z(k);
                    if (d <= depth[o]) continue;
                    depth[o] = d;
                    const at = (values: ArrayLike<number>, e: number) => w[0] * values[3 * i + e] + w[1] * values[3 * j + e] + w[2] * values[3 * k + e];
                    const nx = at(n, 0), ny = at(n, 1), nz = at(n, 2), l = Math.hypot(nx, ny, nz) || 1;
                    const sun = Math.max(0, (nx * SUN[0] + ny * SUN[1] + nz * SUN[2]) / l), sky = 0.85 + (0.15 * nz) / l + 0.2 * (1 - Math.abs(nz / l));
                    for (let e = 0; e < 3; e++) shade[3 * o + e] = at(c, e) * (sky + SUN_WEIGHT * sun);
                }
        }
        for (let o = 0; o < VIEW * VIEW; o++) if (depth[o] > -Infinity) {
            count++;
            for (let e = 0; e < 3; e++) sum[e] += shade[3 * o + e];
        }
    }
    return sum.map(value => value / Math.max(count, 1));
}

/** The triangles of a tree that are not its trunk's. */
function crownTriangles({ index, trunk }: Grown): Uint32Array {
    const out: number[] = [];
    for (let t = 0; t < index.length; t += 3) if (index[t] >= trunk || index[t + 1] >= trunk || index[t + 2] >= trunk) out.push(index[t], index[t + 1], index[t + 2]);
    return Uint32Array.from(out);
}
