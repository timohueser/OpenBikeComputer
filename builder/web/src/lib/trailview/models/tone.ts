import type { Grown } from './trees';

/**
 * The 3 × 3 matrix (columns first) that turns the crown colours of `from` into those of `to`, the
 * same crown in another season, with the least squared error over the crown's vertices; undefined
 * where the crowns differ in shape. A crown's colours blend its season's three ramp colours, so the
 * matrix turns them exactly, but for flecks such as blossom or berries.
 */
export function tone(from: Grown, to: Grown): number[] | undefined {
    if (from.position.length !== to.position.length || from.position.some((p, k) => p !== to.position[k])) return undefined;
    // M = (Σ t sᵀ)(Σ s sᵀ)⁻¹, s a colour in `from` and t in `to`, rows first.
    const ss = new Array(9).fill(0), ts = new Array(9).fill(0);
    for (let v = 3 * from.trunk; v < from.color.length; v += 3) {
        for (let i = 0; i < 3; i++) for (let j = 0; j < 3; j++) {
            ss[3 * i + j] += from.color[v + i] * from.color[v + j];
            ts[3 * i + j] += to.color[v + i] * from.color[v + j];
        }
    }
    const inv = inverse(ss), m = new Array(9).fill(0);
    for (let i = 0; i < 3; i++) for (let j = 0; j < 3; j++) for (let k = 0; k < 3; k++) m[3 * i + j] += ts[3 * i + k] * inv[3 * k + j];
    return [0, 1, 2].flatMap(j => [m[j], m[3 + j], m[6 + j]]);
}

/** Whether `matrix` (columns first) turns every crown colour of `from` within `steps` sRGB steps of 255 of `to`'s. */
export function fits(matrix: number[], from: Grown, to: Grown, steps: number): boolean {
    for (let v = 3 * from.trunk; v < from.color.length; v += 3) {
        for (let i = 0; i < 3; i++) {
            const turned = matrix[i] * from.color[v] + matrix[3 + i] * from.color[v + 1] + matrix[6 + i] * from.color[v + 2];
            if (Math.abs(srgb(Math.max(0, turned)) - srgb(to.color[v + i])) * 255 > steps) return false;
        }
    }
    return true;
}

const srgb = (c: number) => (c <= 0.0031308 ? 12.92 * c : 1.055 * Math.min(c, 1) ** (1 / 2.4) - 0.055);

/** The inverse of a 3 × 3 matrix, rows first. */
function inverse([a, b, c, d, e, f, g, h, i]: number[]): number[] {
    const A = e * i - f * h, B = f * g - d * i, C = d * h - e * g, det = a * A + b * B + c * C;
    return [A, c * h - b * i, b * f - c * e, B, a * i - c * g, c * d - a * f, C, b * g - a * h, a * e - b * d].map(x => x / det);
}
