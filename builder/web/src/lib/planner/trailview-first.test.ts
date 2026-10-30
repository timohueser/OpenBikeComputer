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
    // A valley closed to the east and west. The eye stands 1.5 km up its south flank, which rises on to a summit; the
    // ridge across it, 3 km north, lies below the eye and falls to a plain.
    const flank = ([east, north]: Xy) => 1000 + 0.05 * Math.abs(east) + (north < 0
        ? 2000 * (Math.min(-north, 4500) / 4500) ** 1.6
        : 200 * Math.min(north, 3000) / 3000 - 0.5 * Math.min(Math.max(north - 3000, 0), 1000));

    it('looks across the valley from its flank, not up to the summit', () => {
        expect(farthest(flank, [0, -1500], [180, 0])).toBe(0);
        expect(farthest(flank, [0, -1500], headings)).toBe(0);
    });

    it('looks down a straight valley along its axis', () => {
        const valley = ([east, north]: Xy) => 1000 - 0.01 * east + 0.3 * Math.abs(north);
        expect(farthest(valley, [0, 0], headings)).toBe(90);
    });

    it('looks from a summit over the lowest horizon', () => {
        // Every direction is open to the full reach; the ground falls most steeply to the east.
        const summit = ([east, north]: Xy) => 2000 - 0.1 * Math.hypot(east, north) - 0.2 * Math.max(east, 0);
        expect(farthest(summit, [0, 0], headings)).toBe(90);
    });

    it('turns from a slope that rises behind a bump near the eye', () => {
        // A 5 m bank 50 m to both sides blocks both views at once; the ground beyond it rises to the north only.
        const road = ([, north]: Xy) => (Math.abs(north) > 25 && Math.abs(north) < 75 ? 5 : 0) + Math.max(0.1 * north, 0);
        expect(farthest(road, [0, 0], [0, 180])).toBe(180);
    });

    it('keeps the first heading without terrain', () => {
        expect(farthest(() => NaN, [0, 0], [30, 210])).toBe(30);
    });
});

it('places tile units in metres from the drop, leaves out lines below the surface and on bridges, and keeps a hall around the drop', () => {
    const at: Coordinate = [8.366, 46.958];
    const [x, y] = worldPixel(at, 2 ** 13), u = (x - Math.floor(x)) * EXTENT, v = (y - Math.floor(y)) * EXTENT;
    // About 0.2 m per tile unit: 100 units east is about 20 m.
    const line = (tags: Record<string, string>): Feature => ({ kind: 'path', tags, geometry: 'line', parts: [{ outer: false, xy: new Int32Array([u, v, u + 100, v]), z: null }] });
    // About 400 m across, with the drop in its middle.
    const hall: Feature = { kind: 'building', tags: {}, geometry: 'area', parts: [{ outer: true, xy: new Int32Array([u - 1000, v - 1000, u + 1000, v - 1000, u + 1000, v + 1000, u - 1000, v + 1000]), z: null }] };
    const { lines, houses } = localGeometry([line({ layer: '0' }), line({ layer: '-1' }), line({ bridge: 'yes' }), hall], at);
    expect(lines).toHaveLength(1);
    expect(lines[0][0][0]).toBeCloseTo(0, 0);
    expect(lines[0][1][0]).toBeGreaterThan(15);
    expect(lines[0][1][0]).toBeLessThan(25);
    expect(lines[0][1][1]).toBeCloseTo(0, 0);
    expect(houses).toHaveLength(1);
});

it('follows the curve of a line with an alignment, not its chord', () => {
    const at: Coordinate = [8.366, 46.958];
    const [x, y] = worldPixel(at, 2 ** 13), u = (x - Math.floor(x)) * EXTENT, v = (y - Math.floor(y)) * EXTENT;
    // A bend of 200 units between two tangents 30° off its chord, along y.
    const turn = Math.PI / 6, plan = { along: 0, backward: false, tangent: Float32Array.of(turn, -turn), grade: new Float32Array(2), bank: new Float32Array(2), widen: new Float32Array(2) };
    const road: Feature = { kind: 'road', tags: {}, geometry: 'line', parts: [{ outer: false, xy: Float32Array.of(u, v, u + 200, v), z: Float32Array.of(1000, 1000), plan }] };
    const { lines: [line] } = localGeometry([road], at);
    expect(line.length).toBeGreaterThan(2);
    expect(Math.min(...line.map(([, north]) => north))).toBeLessThan(-2);
});
