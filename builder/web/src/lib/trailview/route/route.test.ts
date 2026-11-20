import { describe, expect, it } from 'vitest';
import { frameAt } from '../frame/frame';
import { viewProjection } from '../frame/camera';
import { nearestRoute, onScreen } from './pick';
import { worldPixel } from '../../planner/layers/mercator';
import { UNITS } from '../frame/space';
import { pickPoints, POINT_WORDS, Route, routePoints, type RouteLine } from './route';

/** The words of each point: x, y, the colour and the number of the route. */
const pointsOf = (words: Uint32Array) => Array.from({ length: words.length / POINT_WORDS }, (_, i) => {
    const ints = new Int32Array(words.buffer, i * POINT_WORDS * 4, POINT_WORDS);
    return { x: ints[0], y: ints[1], colour: words[i * POINT_WORDS + 2], route: words[i * POINT_WORDS + 3] };
});

describe('routePoints', () => {
    it('keeps the vertices, adds points at most 5 m apart, and gives each the colour of the edge that ends at it', () => {
        // About 222 m north, then a repeated vertex, then about 76 m east.
        const points = pointsOf(routePoints([{ line: [[8.3, 46.8], [8.3, 46.802], [8.3, 46.802], [8.301, 46.802]], colours: ['#ff0000', '#00ff00', '#0000ff'] }]));
        const colours = points.map(point => point.colour);
        // The two caps repeat the first and the last point.
        expect(points.length).toBe(1 + Math.ceil(222.4 / 5) + Math.ceil(76.1 / 5) + 2);
        expect(points[0]).toEqual(points[1]);
        expect(points.at(-1)).toEqual(points.at(-2));
        expect(points.every(point => point.route === 1)).toBe(true);
        // The bend is a point of its own and the last red one (red packs as 0x0000ff): the line leaves it east, in blue.
        const bend = colours.lastIndexOf(0x0000ff);
        expect(colours.slice(0, bend + 1).every(c => c === 0x0000ff)).toBe(true);
        expect(colours.slice(bend + 1).every(c => c === 0xff0000)).toBe(true);
        expect(points[bend].x).toBe(points[0].x);
        expect(points[bend + 1].x).toBeGreaterThan(points[0].x);
    });

    it('leaves out a point that rounds onto the point before it, so that the start cap takes the direction of the line', () => {
        // The second vertex lies about 8 mm east of the first, less than one unit of the buffer.
        const points = pointsOf(routePoints([{ line: [[8.3, 46.8], [8.3000001, 46.8], [8.301, 46.8]], colours: ['#ff0000', '#ff0000'] }]));
        expect(points[2].x).toBeGreaterThan(points[1].x);
        points.slice(2, -1).forEach((point, i) => expect(point).not.toEqual(points[i + 1]));
    });

    it('numbers the points of each route, caps included, and parts two routes with two break points; a route of one point keeps its number', () => {
        const red = { line: [[8.3, 46.8], [8.3, 46.8001]], colours: ['#ff0000'] } satisfies RouteLine;
        const blue = { line: [[8.31, 46.8], [8.31, 46.8001]], colours: ['#0000ff'] } satisfies RouteLine;
        const points = pointsOf(routePoints([red, { line: [[8.4, 46.9]], colours: [] }, blue]));
        const routes = points.map(point => point.route), first = routes.indexOf(0);
        expect(routes.slice(0, first).every(route => route === 1)).toBe(true);
        expect(routes.slice(first, first + 2)).toEqual([0, 0]);
        expect(routes.slice(first + 2).every(route => route === 3)).toBe(true);
        // The break points copy the last point of the one route and the first point of the next.
        expect(points[first]).toEqual({ ...points[first - 1], route: 0 });
        expect(points[first + 1]).toEqual({ ...points[first + 2], route: 0 });
        // Each route beside the break ends and starts with its cap.
        expect(points[first - 1]).toEqual(points[first - 2]);
        expect(points[first + 2]).toEqual(points[first + 3]);
        expect(points.length).toBe(2 * pointsOf(routePoints([red])).length + 2);
    });
});

describe('pickPoints', () => {
    const frame = frameAt(8.3, 46.8);
    // 150 m east, then 100 m north.
    const line = [frame.lonLat(0, 0), frame.lonLat(150, 0), frame.lonLat(150, 100)];
    const triples = (points: Float64Array) => Array.from({ length: points.length / 3 }, (_, i) => Array.from(points.subarray(3 * i, 3 * i + 3)));
    const close = (actual: number[][], expected: number[][]) => {
        expect(actual.length).toBe(expected.length);
        expected.forEach((point, i) => point.forEach((value, j) => (Number.isNaN(value) ? expect(actual[i][j]).toBeNaN() : expect(actual[i][j]).toBeCloseTo(value, 6))));
    };

    it('stands a point at each vertex with the height of the profile there, and at each entry between two vertices', () => {
        close(triples(pickPoints({ line, colours: [], profile: { step: 100, heights: [1000, 1010, 1020, 1030] } }, frame)!),
            [[0, 0, 1000], [100, 0, 1010], [150, 0, 1015], [150, 50, 1020], [150, 100, 1030]]);
        // An entry with no height gives no height to itself and to the vertices between the entries beside it.
        close(triples(pickPoints({ line, colours: [], profile: { step: 100, heights: [1000, null, 1020, 1030] } }, frame)!),
            [[0, 0, 1000], [100, 0, NaN], [150, 0, NaN], [150, 50, 1020], [150, 100, 1030]]);
        expect(pickPoints({ line, colours: [] }, frame)).toBeUndefined();
    });

    it('takes the least length of the route that its entries allow when the line is shorter', () => {
        // Five entries: the route is 300 m to 400 m long, so the second entry lies at a third of the line, and the last at its end.
        close(triples(pickPoints({ line, colours: [], profile: { step: 100, heights: [0, 1, 2, 3, 4] } }, frame)!),
            [[0, 0, 0], [250 / 3, 0, 1], [150, 0, 1.8], [150, 50 / 3, 2], [150, 100, 4]]);
    });

    it('places the entries at their share of the route\'s length when the profile has one', () => {
        // The catalog's route is 300 m and its simplified line 250 m: the second entry lies at a third of the line.
        close(triples(pickPoints({ line, colours: [], profile: { step: 100, heights: [0, 1, 2, 3], length: 300 } }, frame)!),
            [[0, 0, 0], [250 / 3, 0, 1], [150, 0, 1.8], [150, 50 / 3, 2], [150, 100, 3]]);
    });

    it('follows a hairpin between two entries: a pointer on the hairpin finds the route, one on the chord between the entries does not', () => {
        // 40 m north, 40 m east and 40 m south: the entries at 0 m and 100 m lie at its start and on its last leg.
        const hairpin = [frame.lonLat(0, 0), frame.lonLat(0, 40), frame.lonLat(40, 40), frame.lonLat(40, 0)];
        const picks = pickPoints({ line: hairpin, colours: [], profile: { step: 100, heights: [0, 0, 0] } }, frame)!;
        // A pixel a metre, from 100 m.
        const screen = new Float32Array(Array.from(picks, (value, k) => (k % 3 === 2 ? 100 : value)));
        expect(nearestRoute([screen], 20, 40, null)).toBe(1);
        // 9 m from the chord between the start and the entry at 100 m, and 20 m from the line.
        expect(nearestRoute([screen], 20, 0, null)).toBeNull();
    });

    it('ends a segment at a gap in the profile: a pointer on the chord across the gap does not find the route', () => {
        // The hairpin with entries 25 m apart; the entries at 50 m and 75 m, on its top, have no height.
        const hairpin = [frame.lonLat(0, 0), frame.lonLat(0, 40), frame.lonLat(40, 40), frame.lonLat(40, 0)];
        const picks = pickPoints({ line: hairpin, colours: [], profile: { step: 25, heights: [0, 0, null, null, 0, 0] } }, frame)!;
        // From 100 m above the middle, straight down.
        const m = viewProjection(0, -90, 1), eye = [20, 20, 100];
        const pixel = (east: number, north: number) => onScreen(m, eye, 1000, 1000, new Float64Array([east, north, 0]));
        const screen = onScreen(m, eye, 1000, 1000, picks), at = (east: number, north: number) => nearestRoute([screen], pixel(east, north)[0], pixel(east, north)[1], null);
        expect(at(40, 10)).toBe(1);
        // Between the entries at 25 m and 100 m, 20 m from the line.
        expect(at(20, 22.5)).toBeNull();
    });

    it('gives a route with no profile the drawn heights at each 100 m of its line and at its end', async () => {
        const route = new Route(), colours = ['#ff0000', '#ff0000'], profiled = { line, colours, profile: { step: 100, heights: [0, 0, 0, 0] } };
        let asked: Float64Array | undefined;
        await route.set([profiled, { line, colours }], frame, async points => {
            asked = points;
            return new Float32Array([1000, 1010, NaN, 1025]);
        });
        const spots = [[0, 0], [100, 0], [150, 50], [150, 100]].map(([east, north]) => worldPixel(frame.lonLat(east, north), UNITS));
        Array.from(asked!).forEach((value, k) => expect(value).toBeCloseTo(spots[k >> 1][k & 1], 3));
        expect(route.picks[0]).toEqual(pickPoints(profiled, frame));
        close(triples(route.picks[1]!), [[0, 0, 1000], [100, 0, 1010], [150, 0, NaN], [150, 50, NaN], [150, 100, 1025]]);
    });

    it('keeps the routes of a later set when the drawn heights of an earlier one arrive', async () => {
        const route = new Route();
        let resolve!: (heights: Float32Array) => void;
        const first = route.set([{ line, colours: ['#ff0000', '#ff0000'] }], frame, () => new Promise(done => { resolve = done; }));
        void route.set([], frame);
        resolve(new Float32Array(4));
        await first;
        expect(route.picks).toEqual([]);
    });
});

it('clears the bold route with new routes', () => {
    const route = new Route();
    route.setBold(2);
    route.set([], frameAt(8.3, 46.8));
    expect(route.bold).toBeNull();
});
