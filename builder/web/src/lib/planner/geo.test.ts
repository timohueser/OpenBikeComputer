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
        const hit = (quiet: number[], y: number) => nearestOnScreen(lines, ([x, y]) => ({ x, y }), ({ x, y }) => [x, y], { x: 50, y }, 10, line => quiet.includes(line.day))?.line.day;
        expect(hit([3], 1)).toBe(18);
        // A quiet line under the pointer wins over a day in colour 8 px away.
        expect(hit([3, 18], 0)).toBe(3);
        // With no quiet line, the nearest line wins, the earlier one on a tie.
        expect(hit([], 1)).toBe(3);
    });

    // A Web Mercator view without pitch: world pixels at `zoom`, turned by `bearing` around the screen centre.
    const view = (center: Coordinate, zoom: number, bearing: number) => {
        const size = 512 * 2 ** zoom, cos = Math.cos(bearing * Math.PI / 180), sin = Math.sin(bearing * Math.PI / 180);
        const world = ([lon, lat]: Coordinate) => [(lon + 180) / 360 * size, (.5 - Math.log(Math.tan(Math.PI / 4 + lat * Math.PI / 360)) / (2 * Math.PI)) * size];
        const [cx, cy] = world(center);
        return {
            project: (c: Coordinate) => { const [x, y] = world(c); return { x: 400 + (x - cx) * cos - (y - cy) * sin, y: 300 + (x - cx) * sin + (y - cy) * cos }; },
            unproject: ({ x, y }: { x: number; y: number }): Coordinate => {
                const wx = cx + (x - 400) * cos + (y - 300) * sin, wy = cy - (x - 400) * sin + (y - 300) * cos;
                return [wx / size * 360 - 180, Math.atan(Math.sinh(Math.PI * (1 - 2 * wy / size))) * 180 / Math.PI];
            },
        };
    };

    // The search over every point of every line, without boxes.
    function plain<L extends { coordinates: Coordinate[] }>(lines: L[], project: (c: Coordinate) => { x: number; y: number }, point: { x: number; y: number }, reach: number, quiet: (line: L) => boolean) {
        const best: ({ line: L; x: number; y: number; pixels: number } | null)[] = [null, null];
        for (const line of lines) {
            const kind = quiet(line) ? 1 : 0, projected = line.coordinates.map(project);
            for (let i = 1; i < projected.length; i++) {
                const a = projected[i - 1], b = projected[i], dx = b.x - a.x, dy = b.y - a.y;
                const t = Math.max(0, Math.min(1, ((point.x - a.x) * dx + (point.y - a.y) * dy) / (dx * dx + dy * dy || 1)));
                const pixels = Math.hypot(point.x - a.x - dx * t, point.y - a.y - dy * t);
                if (pixels <= reach && (!best[kind] || pixels < best[kind].pixels)) best[kind] = { line, x: a.x + dx * t, y: a.y + dy * t, pixels };
            }
        }
        const [colour, grey] = best;
        const found = colour && (!grey || colour.pixels <= grey.pixels + 2) ? colour : grey;
        return found && { line: found.line, x: found.x, y: found.y };
    }

    it('finds the same point as a search over every point, at any zoom and bearing', () => {
        let seed = 7;
        const random = () => (seed = (seed * 16807) % 2147483647) / 2147483647;
        const lines: { day: number; coordinates: Coordinate[] }[] = [];
        for (let day = 1; day <= 12; day++) {
            let at: Coordinate = [8 + random() * .2, 47 + random() * .2];
            const coordinates: Coordinate[] = [at];
            for (let i = 0; i < 40; i++) coordinates.push(at = [at[0] + (random() - .5) * .01, at[1] + (random() - .5) * .01]);
            lines.push({ day, coordinates });
        }
        // Days that share a road, in both directions.
        lines.push({ day: 13, coordinates: lines[2].coordinates.slice(5, 30) }, { day: 14, coordinates: [...lines[4].coordinates].reverse() });
        const quiet = (line: { day: number }) => line.day % 3 === 0;
        let hits = 0;
        for (const [zoom, bearing] of [[9, 0], [13, 0], [13, 37], [16, -120]]) {
            const { project, unproject } = view(lines[2].coordinates[10], zoom, bearing);
            const points: { x: number; y: number }[] = [];
            for (let i = 0; i < 300; i++) points.push({ x: random() * 800, y: random() * 600 });
            // Positions near the corners of each line's box and near the line points.
            for (const { coordinates } of lines) {
                const lons = coordinates.map(c => c[0]), lats = coordinates.map(c => c[1]);
                for (const corner of [[Math.min(...lons), Math.min(...lats)], [Math.max(...lons), Math.max(...lats)]] as Coordinate[]) {
                    const p = project(corner);
                    for (const [dx, dy] of [[-8, -8], [8, 8], [-7, 7], [11, 0]]) points.push({ x: p.x + dx, y: p.y + dy });
                }
                for (const c of coordinates.slice(0, 10)) { const p = project(c); points.push({ x: p.x + random() * 20 - 10, y: p.y + random() * 20 - 10 }); }
            }
            for (const point of points) {
                const expected = plain(lines, project, point, 10, quiet);
                const found = nearestOnScreen(lines, project, unproject, point, 10, quiet);
                expect(found && { line: found.line, x: found.x, y: found.y }).toEqual(expected);
                if (expected) hits++;
            }
        }
        expect(hits).toBeGreaterThan(500);
    });

    it('finds a line from a pointer outside its box', () => {
        const line = { coordinates: [[8, 47], [8.01, 47]] as Coordinate[] };
        const { project, unproject } = view([8, 47], 14, 0);
        const end = project([8.01, 47]);
        // 9.5 px from the line end, beyond the end, along each screen axis and along the line.
        for (const point of [{ x: end.x + 9.5, y: end.y }, { x: end.x, y: end.y + 9.5 }, { x: end.x + 6.7, y: end.y + 6.7 }]) {
            const at = unproject(point);
            expect(at[0] > 8.01 || at[1] !== 47).toBe(true);
            expect(nearestOnScreen([line], project, unproject, point, 10, () => false)?.line).toBe(line);
        }
    });
});
