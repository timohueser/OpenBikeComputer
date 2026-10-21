import { worldPixel } from '../../planner/layers/mercator';
import { EARTH_RADIUS, tileLonLat, type Frame } from './frame';

/**
 * World units: 2^30 across the Web Mercator world, so every grid vertex of every terrain level lies
 * on a whole unit. Shaders get whole units about an anchor, so a float never holds a world coordinate.
 */
export const UNITS = 2 ** 30;

/**
 * The eye in world units: `whole` is the anchor, `part` the rest of a unit. LOD space is metres east
 * and north of the eye along the Mercator axes, at the scale of the eye's latitude: `unit` metres per unit.
 */
export interface Anchor { whole: [number, number]; part: [number, number]; unit: number }

const RAD = Math.PI / 180;

export function anchorAt(lon: number, lat: number): Anchor {
    const [x, y] = worldPixel([lon, lat], UNITS), whole: [number, number] = [Math.floor(x), Math.floor(y)];
    return { whole, part: [x - whole[0], y - whole[1]], unit: 2 * Math.PI * EARTH_RADIUS * Math.cos(lat * RAD) / UNITS };
}

/** LOD space of a point in world units. */
export function lodSpace(anchor: Anchor, x: number, y: number): [number, number] {
    return [(x - anchor.whole[0] - anchor.part[0]) * anchor.unit, -(y - anchor.whole[1] - anchor.part[1]) * anchor.unit];
}

/** The cubic `c` (10 coefficients of 1, x, y, x², xy, y², x³, x²y, xy², y³) at (x, y). */
export function cubic(c: ArrayLike<number>, x: number, y: number, at = 0): number {
    return c[at] + x * (c[at + 1] + x * (c[at + 3] + x * c[at + 6] + y * c[at + 7])) + y * (c[at + 2] + y * (c[at + 5] + y * c[at + 9] + x * c[at + 8])) + c[at + 4] * x * y;
}

/** Sample points of the fit: a 7 × 7 grid this many metres apart, about the eye. */
const FIT_STEP = 1500;

/**
 * The cubic that takes LOD space to render space (frame metres about the eye): x then y, 20
 * coefficients. It is fitted about the eye, so the error grows with d⁴/R³: 0.5 mm at 30 km, 7 cm at
 * 100 km and 6 m at 300 km.
 */
export function renderMap(frame: Frame, anchor: Anchor): Float64Array {
    const eye = [anchor.whole[0] + anchor.part[0], anchor.whole[1] + anchor.part[1]];
    const [eyeEast, eyeNorth] = frame.local(...tileLonLat(eye[0], eye[1], 30));
    const lod: [number, number][] = [], render: [number, number][] = [];
    for (let j = -3; j <= 3; j++) for (let i = -3; i <= 3; i++) {
        const s: [number, number] = [i * FIT_STEP, j * FIT_STEP];
        const [east, north] = frame.local(...tileLonLat(eye[0] + s[0] / anchor.unit, eye[1] - s[1] / anchor.unit, 30));
        lod.push(s);
        render.push([east - eyeEast, north - eyeNorth]);
    }
    const out = new Float64Array(20);
    for (let axis = 0; axis < 2; axis++) out.set(leastSquares(lod, render.map(point => point[axis])), 10 * axis);
    return out;
}

/** The cubic through the points (x, y) → v, by normal equations in units of FIT_STEP. */
function leastSquares(points: [number, number][], values: number[]): number[] {
    const basis = (x: number, y: number) => [1, x, y, x * x, x * y, y * y, x * x * x, x * x * y, x * y * y, y * y * y];
    const a = Array.from({ length: 10 }, () => new Array<number>(11).fill(0));
    points.forEach(([x, y], k) => {
        const b = basis(x / FIT_STEP, y / FIT_STEP);
        for (let i = 0; i < 10; i++) {
            for (let j = 0; j < 10; j++) a[i][j] += b[i] * b[j];
            a[i][10] += b[i] * values[k];
        }
    });
    for (let i = 0; i < 10; i++) {
        const pivot = a.slice(i).reduce((best, row, k) => (Math.abs(row[i]) > Math.abs(a[best][i]) ? i + k : best), i);
        [a[i], a[pivot]] = [a[pivot], a[i]];
        for (let k = 0; k < 10; k++) {
            if (k === i) continue;
            const f = a[k][i] / a[i][i];
            for (let j = i; j <= 10; j++) a[k][j] -= f * a[i][j];
        }
    }
    const degree = [0, 1, 1, 2, 2, 2, 3, 3, 3, 3];
    return a.map((row, i) => row[10] / row[i] / FIT_STEP ** degree[i]);
}
