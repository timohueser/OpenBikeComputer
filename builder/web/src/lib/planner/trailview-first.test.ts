import { describe, expect, it } from 'vitest';
import { EXTENT, type Feature } from '../trailview/stream/features';
import type { Coordinate } from './geo';
import { worldPixel } from './layers/mercator';
import { farthest, localGeometry, outside, routeView, snap, type Xy } from './trailview-first';

describe('snap', () => {
    const lines: Xy[][] = [[[-50, 10], [50, 10]], [[-50, -5], [50, -5]]];

    it('moves onto the nearest line within the radius, along its direction', () => {
        expect(snap([0, 0], lines, 12)).toEqual({ at: [0, -5], along: 90 });
    });

    it('leaves a point farther than the radius', () => {
        expect(snap([0, 0], lines, 4)).toBeUndefined();
    });
});

describe('outside', () => {
    const square = (west: number): Xy[][] => [[[west, -5], [west + 10, -5], [west + 10, 5], [west, 5]]];

    it('steps out by the nearest wall', () => {
        expect(outside([1, 0], [square(-5)], 2)).toEqual([7, 0]);
        expect(outside([20, 0], [square(-5)], 2)).toEqual([20, 0]);
    });

    it('leaves a row house by a wall that no other house shares', () => {
        const [east, north] = outside([4, 0], [square(-5), square(5)], 2);
        expect(east).toBeCloseTo(4);
        expect(Math.abs(north)).toBeCloseTo(7);
    });
});

describe('routeView', () => {
    const route: Coordinate[] = [[8.30, 47.00], [8.31, 47.00], [8.32, 47.00]];

    it('looks along the route in its coordinate order', () => {
        const view = routeView(route, [8.315, 47.0001], 20)!;
        expect(view.heading).toBeCloseTo(90);
        expect(view.lat).toBeCloseTo(47.00, 6);
        expect(routeView([...route].reverse(), [8.315, 47.0001], 20)!.heading).toBeCloseTo(270);
    });

    it('is not on a route farther than the radius', () => {
        expect(routeView(route, [8.315, 47.0001], 5)).toBeUndefined();
    });
});

describe('farthest', () => {
    const headings = Array.from({ length: 36 }, (_, i) => i * 10);

    it('looks out of a valley that opens to the east', () => {
        // Walls rise 300 m to the north and south, and a ridge closes the valley 500 m to the west.
        const valley = ([east, north]: Xy) => Math.min(Math.abs(north), 300) + (east < 0 ? Math.min(-east, 500) : 0);
        expect(farthest(valley, [0, 0], headings)).toBe(90);
    });

    it('breaks a tie of distance toward the lowest horizon', () => {
        // A slope that rises to the north shows its ground to the full reach in every direction.
        expect(farthest(([, north]) => north / 10, [0, 0], headings)).toBe(180);
    });

    it('keeps the first heading without terrain', () => {
        expect(farthest(() => NaN, [0, 0], [30, 210])).toBe(30);
    });
});

it('places tile units in metres from the drop and leaves out lines below the surface', () => {
    const at: Coordinate = [8.366, 46.958];
    const [x, y] = worldPixel(at, 2 ** 13), u = (x - Math.floor(x)) * EXTENT, v = (y - Math.floor(y)) * EXTENT;
    // About 0.2 m per tile unit: 100 units east is about 20 m.
    const line = (layer: string): Feature => ({ kind: 'path', tags: { layer }, geometry: 'line', parts: [{ outer: false, xy: new Int32Array([u, v, u + 100, v]), z: null }] });
    const { lines } = localGeometry([line('0'), line('-1')], at);
    expect(lines).toHaveLength(1);
    expect(lines[0][0][0]).toBeCloseTo(0, 0);
    expect(lines[0][1][0]).toBeGreaterThan(15);
    expect(lines[0][1][0]).toBeLessThan(25);
    expect(lines[0][1][1]).toBeCloseTo(0, 0);
});
