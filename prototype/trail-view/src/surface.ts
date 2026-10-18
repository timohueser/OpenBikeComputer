/** The ground the page draws, read on the CPU by the vertex shader's level rule. */

/** Peak View's constant: curvature with refraction, as metres of drop per square metre of range. */
export const CURVATURE = 0.87 / (2 * 6_371_000);

export interface Surface {
  levels: number;
  half0: number;
  /** Metres the levels' values sit above heights relative to this surface's own eye. */
  lift: number;
  /** Height above the eye at (x, y) on level k, blended toward k + 1 over its outer tenth. */
  level(k: number, x: number, y: number): number;
  /** Height above the eye at (x, y) on the finest level that holds it; NaN beyond the last. */
  at(x: number, y: number): number;
}

const smoothstep = (edge0: number, edge1: number, x: number) => {
  const t = Math.min(Math.max((x - edge0) / (edge1 - edge0), 0), 1);
  return t * t * (3 - 2 * t);
};

/**
 * Inside BLANKET[0] metres instanced trees stand on the bare DTM; past BLANKET[1] the canopy lies
 * on it as a raised blanket, which meets GLO-30, a surface model, without a step.
 */
export const BLANKET = [800, 1200];

/**
 * The sampling rule the vertex shader uses, over levels with row 0 north. Heights are relative to
 * an eye `above` metres over this surface at the origin, so a coarse surface never buries the eye.
 * `canopy` holds canopy heights for the first levels, laid on as the blanket.
 */
export function surface(levels: Float32Array[], half0: number, above: number, canopy: Float32Array[] = []): Surface {
  const last = levels.length - 1;
  // Each level is square; heights and canopy have their own sizes.
  const bilinear = (data: Float32Array, k: number, x: number, y: number) => {
    const size = Math.sqrt(data.length);
    const half = half0 * 2 ** k;
    const scale = size / (2 * half);
    const u = (x + half) * scale - 0.5;
    const v = (half - y) * scale - 0.5;
    const i = Math.min(Math.max(Math.floor(u), 0), size - 2);
    const j = Math.min(Math.max(Math.floor(v), 0), size - 2);
    const fu = Math.min(Math.max(u - i, 0), 1);
    const fv = Math.min(Math.max(v - j, 0), 1);
    const at = j * size + i;
    const top = data[at] + (data[at + 1] - data[at]) * fu;
    const bottom = data[at + size] + (data[at + size + 1] - data[at + size]) * fu;
    return top + (bottom - top) * fv;
  };
  const height = (k: number, x: number, y: number) =>
    bilinear(levels[k], k, x, y) +
    (k < canopy.length ? bilinear(canopy[k], k, x, y) * smoothstep(BLANKET[0], BLANKET[1], Math.hypot(x, y)) : 0);
  const raw = (k: number, x: number, y: number) => {
    const h = height(k, x, y);
    if (k === last) return h;
    const w = smoothstep(0.9, 1, Math.max(Math.abs(x), Math.abs(y)) / (half0 * 2 ** k));
    return w > 0 ? h + (height(k + 1, x, y) - h) * w : h;
  };
  const lift = raw(0, 0, 0) + above;
  const level = (k: number, x: number, y: number) => raw(k, x, y) - lift;
  return {
    levels: levels.length,
    half0,
    lift,
    level,
    at(x, y) {
      const m = Math.max(Math.abs(x), Math.abs(y));
      const k = Math.max(0, Math.ceil(Math.log2(Math.max(m, 1) / half0)));
      return k > last ? NaN : level(k, x, y);
    },
  };
}
