import { expect, it } from 'vitest';
import { nearestRoute } from './pick';

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
