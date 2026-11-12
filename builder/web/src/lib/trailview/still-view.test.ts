import { describe, expect, it } from 'vitest';
import { latitudeAt } from '../planner/layers/mercator';
import { CURVATURE, frameAt } from './frame/frame';
import { chooseView, type Camera, type HeightGrid } from './still-view';

const CENTRE: [number, number] = [8.4, 46.8];
const frame = frameAt(...CENTRE);
const SIZE = { width: 1920, height: 1280 };

/** A grid of 1,024 × 1,024 cells of a zoom about CENTRE, its heights from metres east and north of it. */
function grid(zoom: number, height: (east: number, north: number) => number, cells = 1024): HeightGrid {
    const size = 512 * 2 ** zoom, x0 = Math.round((CENTRE[0] + 180) / 360 * size - cells / 2), y0 = Math.round((0.5 - Math.asinh(Math.tan(CENTRE[1] * Math.PI / 180)) / (2 * Math.PI)) * size - cells / 2);
    const heights = new Float32Array(cells * cells);
    for (let j = 0; j < cells; j++) for (let i = 0; i < cells; i++) {
        heights[j * cells + i] = height(...frame.local((x0 + i + 0.5) / size * 360 - 180, latitudeAt(y0 + j + 0.5, size)));
    }
    return { size, x0, y0, width: cells, height: cells, heights };
}

/** A line from metres east and north of CENTRE. */
const line = (points: [number, number][]) => points.map(([east, north]) => frame.lonLat(east, north));
/** A valley that runs north for 12 km: a floor at 1000 m, 800 m wide, between walls that rise 1500 m. Its road winds on the floor. */
const valley = (east: number) => 1000 + 1500 * Math.min(1, Math.max(0, (Math.abs(east) - 400) / 1500));
const road = line(Array.from({ length: 61 }, (_, i) => [250 * Math.sin(i / 4), -6000 + 200 * i]));

/** Whether two bearings in degrees lie within `limit` of each other. */
const near = (a: number, b: number, limit: number) => Math.abs(((a - b) % 360 + 540) % 360 - 180) < limit;

/** The points as the engine draws them from the view, from −1 to 1 across the frame: in the frame of the eye, the ground drops with the curvature. */
function screen(view: Camera, points: [number, number][], height: number): [number, number][] {
    const eye = frameAt(view.lon, view.lat), h = view.heading * Math.PI / 180, p = view.pitch * Math.PI / 180, tan = Math.tan(Math.PI / 6);
    const forward = [Math.sin(h) * Math.cos(p), Math.cos(h) * Math.cos(p), Math.sin(p)], right = [Math.cos(h), -Math.sin(h), 0];
    const up = [-Math.sin(h) * Math.sin(p), -Math.cos(h) * Math.sin(p), Math.cos(p)];
    return points.map(([lon, lat]) => {
        const [x, y] = eye.local(lon, lat), v = [x, y, height - view.height - CURVATURE * (x * x + y * y)];
        const dot = (a: number[]) => a[0] * v[0] + a[1] * v[1] + a[2] * v[2], depth = dot(forward);
        return depth > 0 ? [dot(right) / depth / tan / (SIZE.width / SIZE.height), dot(up) / depth / tan] : [Infinity, Infinity];
    });
}

describe('the cover view', () => {
    it('looks into a valley over its walls, and never within 60° of the sun', () => {
        for (const sun of [90, 0, 135, 270]) {
            const plain = chooseView(road, grid(10, () => 1000), { ...SIZE, sun })!, view = chooseView(road, grid(10, valley), { ...SIZE, sun })!;
            // The walls hide the road from the lowest tilts.
            expect(view.pitch, `sun ${sun}`).toBeLessThan(plain.pitch - 10);
            expect(near(view.heading, sun, 60), `sun ${sun}: heading ${view.heading}`).toBe(false);
            expect(near(plain.heading, sun, 60), `sun ${sun}: heading ${plain.heading}`).toBe(false);
        }
    });

    it('holds each point of a long route inside the frame as the engine draws it, so the height of the eye is above sea level', () => {
        // 120 km across: from the eye, the curvature drops the ground by hundreds of metres.
        const route = line([[-60_000, -30_000], [0, 0], [60_000, 20_000]]);
        for (const sun of [180, 90]) {
            const view = chooseView(route, grid(8, () => 0), { ...SIZE, sun })!;
            expect(view.mode).toBe('fly');
            for (const [x, y] of screen(view, route, 0)) {
                expect(Math.abs(x)).toBeLessThan(0.905);
                expect(Math.abs(y)).toBeLessThan(0.905);
            }
        }
    });

    it('searches a grid of 1,024 × 1,024 cells in less than 200 ms', () => {
        const terrain = grid(10, (east, north) => valley(east) + 300 * Math.sin(north / 900)), start = performance.now();
        expect(chooseView(road, terrain, { ...SIZE, sun: 90 })).toBeDefined();
        const ms = performance.now() - start;
        console.log(`The search of a grid of 1,024 × 1,024 cells took ${ms.toFixed(0)} ms.`);
        expect(ms).toBeLessThan(200);
    });
});
