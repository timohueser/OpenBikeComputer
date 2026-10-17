/**
 * The skyline as Peak View casts it: per bearing, the highest elevation angle over the curved,
 * refracted surface. It measures the photographs and decides which peaks are in sight.
 */

import type { Peak } from './bake';

/** Peak View's constant: curvature with refraction, as metres of drop per square metre of range. */
export const CURVATURE = 0.87 / (2 * 6_371_000);
/** Quarter-degree bearings, the grid the photographs' columns are on. */
export const BINS = 1440;

const MAX_PEAKS = 64;
const SECTORS = 16;
/** Running-maximum stops for peak visibility: 8 per octave from 10 m. */
const STOP_R0 = 10;
const STOPS_PER_OCTAVE = 8;


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
export function surface(levels: Float32Array[], size: number, half0: number, above: number, canopy: Float32Array[] = []): Surface {
  const last = levels.length - 1;
  const bilinear = (data: Float32Array, k: number, x: number, y: number) => {
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

export interface Skyline {
  /** The highest elevation angle per quarter-degree bearing, in degrees. */
  angle: Float32Array;
  /** The running maximum tangent per bearing at each stop, `stops` per bearing. */
  profile: Float32Array;
  stops: number;
}

/** Cast every quarter-degree bearing, half a level pixel per step, to the edge of the last level. */
export function skyline(ground: Surface, size: number): Skyline {
  const reach = ground.half0 * 2 ** (ground.levels - 1);
  const stops = Math.ceil(Math.log2((reach * Math.SQRT2) / STOP_R0) * STOPS_PER_OCTAVE) + 1;
  const angle = new Float32Array(BINS);
  const profile = new Float32Array(BINS * stops);
  for (let b = 0; b < BINS; b++) {
    const bearing = (b / BINS) * 2 * Math.PI;
    const east = Math.sin(bearing);
    const north = Math.cos(bearing);
    const chebyshev = Math.max(Math.abs(east), Math.abs(north));
    let best = -Infinity;
    let stop = 0;
    let next = STOP_R0;
    let r = 2;
    for (let k = 0; k < ground.levels; k++) {
      const half = ground.half0 * 2 ** k;
      const end = half / chebyshev;
      const step = half / size;
      for (; r <= end; r += step) {
        while (r > next && stop < stops) {
          profile[b * stops + stop++] = best;
          next = STOP_R0 * 2 ** (stop / STOPS_PER_OCTAVE);
        }
        const tangent = (ground.level(k, r * east, r * north) - CURVATURE * r * r) / r;
        if (tangent > best) best = tangent;
      }
    }
    while (stop < stops) profile[b * stops + stop++] = best;
    angle[b] = (Math.atan(best) * 180) / Math.PI;
  }
  return { angle, profile, stops };
}

/** The skyline at any bearing, between its two quarter-degree neighbours. */
export function skylineAt(sky: Skyline, bearing: number): number {
  const position = ((((bearing % 360) + 360) % 360) * BINS) / 360;
  const low = Math.floor(position);
  const a = sky.angle[low % BINS];
  const b = sky.angle[(low + 1) % BINS];
  return a + (b - a) * (position - low);
}

/** Root-mean-square difference to a photograph's columns, in degrees. */
export function rms(sky: Skyline, columns: [number, number][]): number {
  const sum = columns.reduce((total, [bearing, elevation]) => total + (skylineAt(sky, bearing) - elevation) ** 2, 0);
  return Math.sqrt(sum / columns.length);
}

export interface Sighted {
  peak: Peak;
  bearing: number;
  /** Elevation angle of the drawn surface under the peak, in degrees. */
  angle: number;
  distance: number;
}

/**
 * The peaks no nearer terrain hides, chosen by Peak View's rule: the tallest of each 22.5° sector
 * first, then the rest by angle above the horizontal, at most 64.
 */
export function sightedPeaks(peaks: Peak[], ground: Surface, sky: Skyline): Sighted[] {
  const sighted: Sighted[] = [];
  for (const peak of peaks) {
    const distance = Math.hypot(peak.x, peak.y);
    const height = ground.at(peak.x, peak.y);
    if (distance < 30 || Number.isNaN(height)) continue;
    const tangent = (height - CURVATURE * distance * distance) / distance;
    const bearing = ((Math.atan2(peak.x, peak.y) * 180) / Math.PI + 360) % 360;
    // The summit's own flank, the last 5 % of the way, does not hide it.
    const stop = Math.floor(Math.log2((0.95 * distance) / STOP_R0) * STOPS_PER_OCTAVE);
    const bin = Math.round((bearing * BINS) / 360) % BINS;
    if (stop >= 0 && tangent < sky.profile[bin * sky.stops + Math.min(stop, sky.stops - 1)]) continue;
    sighted.push({ peak, bearing, angle: (Math.atan(tangent) * 180) / Math.PI, distance });
  }
  const tallest = new Map<number, Sighted>();
  const height = (s: Sighted) => s.peak.ele ?? -Infinity;
  for (const s of sighted) {
    const sector = Math.floor(s.bearing / (360 / SECTORS)) % SECTORS;
    const held = tallest.get(sector);
    if (!held || height(s) > height(held)) tallest.set(sector, s);
  }
  const chosen = new Set(tallest.values());
  for (const s of [...sighted].sort((a, b) => b.angle - a.angle)) {
    if (chosen.size >= MAX_PEAKS) break;
    chosen.add(s);
  }
  return [...chosen];
}
