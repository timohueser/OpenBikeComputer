import { describe, expect, it } from 'vitest';
import { viewProjection } from '../frame/camera';
import { nearestRoute, onScreen } from './pick';

/** Pick points as x and y in CSS pixels and the distance from the eye. */
const points = (...values: number[]) => new Float32Array(values);

it('finds a route on a segment between two far points', () => {
    const routes = [undefined, points(0, 100, 500, 1000, 140, 900)];
    expect(nearestRoute(routes, 500, 128, null)).toBe(2);
});

it('gives the bold route of two on one path, else the one nearer the eye, else the lower number', () => {
    const far = points(0, 100, 800, 1000, 100, 800), near = points(0, 102, 400, 1000, 102, 400);
    expect(nearestRoute([far, near], 500, 101, null)).toBe(2);
    expect(nearestRoute([far, near], 500, 101, 1)).toBe(1);
    expect(nearestRoute([near, near], 500, 101, null)).toBe(1);
});

it('gives null with no segment within 12 pixels', () => {
    expect(nearestRoute([points(0, 100, 500, 1000, 100, 500)], 500, 112.5, null)).toBeNull();
    expect(nearestRoute([], 500, 100, null)).toBeNull();
});

it('ends a segment at a point behind the eye', () => {
    const line = points(0, 100, 500, NaN, NaN, NaN, 1000, 100, 500, 1000, 300, 500);
    expect(nearestRoute([line], 500, 100, null)).toBeNull();
    expect(nearestRoute([line], 1000, 200, null)).toBe(1);
});

describe('onScreen', () => {
    // An eye 1.3 m above a route, looking north along it; the route draws 0.3 m above its points.
    const m = viewProjection(0, 0, 1), eye = [0, 0, 1.3];
    const project = (...values: number[]) => onScreen(m, eye, 1000, 1000, new Float64Array(values));
    const pointer = (north: number, height: number) => project(0, north, height).subarray(0, 2);

    it('keeps the part in front of the eye of a segment from behind the eye', () => {
        const [x, y] = pointer(10, 0);
        expect(nearestRoute([project(0, -5, 0, 0, 50, 0)], x, y, null)).toBe(1);
    });

    it('gives the route the eye stands on its own distance near the eye, ahead of a farther route on the same pixel', () => {
        // Route 2 crosses 60 m ahead, on the pixel of the place of route 1 20 m ahead.
        const [x, y] = pointer(20, 0), second = project(-50, 60, -2, 50, 60, -2);
        expect(nearestRoute([undefined, second], x, y, null)).toBe(2);
        expect(nearestRoute([project(0, -5, 0, 0, 100, 0), second], x, y, null)).toBe(1);
    });
});
