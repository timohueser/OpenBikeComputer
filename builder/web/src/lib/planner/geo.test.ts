import { describe, expect, it } from 'vitest';
import { cumulative, kilometres, kmPerDegree, nearestOnLine, nearestOnScreen, nearestProgress, routeDistance, routeSlice, type Coordinate } from './geo';

describe('route slices', () => {
    it('interpolates boundaries and retains the vertices between them', () => {
        const coordinates: [number, number][] = [[0, 0], [1, 0], [2, 0], [3, 0]];
        const slice = routeSlice(coordinates, .2, .8);
        expect(slice[0][0]).toBeCloseTo(.6, 10);
        expect(slice.at(-1)![0]).toBeCloseTo(2.4, 10);
        expect(slice.slice(1, -1)).toEqual([[1, 0], [2, 0]]);
        expect(cumulative(slice).at(-1)).toBeCloseTo(cumulative(coordinates).at(-1)! * .6, 7);
        expect(routeSlice(coordinates, .8, .2)).toEqual([...slice].reverse());
    });
    it('freezes a measured line, so an in-place edit fails', () => {
        const line: Coordinate[] = [[0, 0], [1, 0]];
        cumulative(line);
        expect(() => line.push([2, 0])).toThrow(TypeError);
        expect(() => { line[1][0] = 2; }).toThrow(TypeError);
    });
});

describe('nearest position', () => {
    it('measures in kilometres to the very ends of the line', () => {
        // About 300 km along 47° N; the visit is 300 m along and 1 km off the line.
        const line: Coordinate[] = [[7, 47], [11, 47]];
        const visit: Coordinate = [7 + .3 / (kmPerDegree * Math.cos(47 * Math.PI / 180)), 47.009];
        expect(nearestProgress(line, visit) * cumulative(line).at(-1)!).toBeCloseTo(.3, 2);
        // On a diagonal at 60° N, no position of the line is nearer than the one found.
        const diagonal: Coordinate[] = [[10, 60], [12, 61]], point: Coordinate = [11, 60.2];
        const off = kilometres(nearestOnLine(diagonal, point).at, point);
        for (let s = 0; s <= 100; s++) expect(off).toBeLessThanOrEqual(kilometres([10 + s / 50, 60 + s / 100], point) + 1e-3);
    });
});

// At 48° N, one kilometre is 0.00899° of latitude and 0.01344° of longitude.
describe('route distance', () => {
    const line = Array.from({ length: 100 }, (_, i): Coordinate => [8 + i / 99, 48]);
    const distance = routeDistance(line, 5);

    it('measures a place beside any part of the route and drops one beyond the corridor', () => {
        expect(distance([8.9, 48 + 4 * .00899])).toBeCloseTo(4, 2);
        expect(distance([8.9, 48 + 6 * .00899])).toBe(Infinity);
    });

    it('measures past the route end to the end point', () => {
        expect(distance([9 + 4 * .01344, 48])).toBeCloseTo(4, 2);
        expect(distance([9 + 6 * .01344, 48])).toBe(Infinity);
    });
});

describe('line under the pointer', () => {
    it('takes the day in colour where it shares a road with a quiet day, else the nearest line', () => {
        const road: Coordinate[] = [[0, 0], [100, 0]];
        const lines = [{ day: 3, coordinates: road }, { day: 18, coordinates: [...road].reverse() }, { day: 19, coordinates: [[0, 8], [100, 8]] as Coordinate[] }];
        const hit = (quiet: number[], y: number) => nearestOnScreen(lines, ([x, y]) => ({ x, y }), { x: 50, y }, 10, line => quiet.includes(line.day))?.line.day;
        expect(hit([3], 1)).toBe(18);
        // A quiet line under the pointer wins over a day in colour 8 px away.
        expect(hit([3, 18], 0)).toBe(3);
        // With no quiet line, the nearest line wins, the earlier one on a tie.
        expect(hit([], 1)).toBe(3);
    });
});
