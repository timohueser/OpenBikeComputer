import { describe, expect, it } from 'vitest';
import { calculateLine, type EngineRoute } from './routing';
import { emptyTrip, planView, type RoutePoint, type Trip } from './editor';
import type { LegCache } from './route-legs';
import { gapLabel, routeGaps } from './route-gaps';

// The router attaches each point to the nearest road: here the equator, from 0° to 0.02°.
const road: EngineRoute = {
    package: 'p', profile: 'hiking', geometry: [[0, 0], [0.01, 0], [0.02, 0]],
    elevation: [null, null, null], elapsed: [0, 1, 2], edges: {},
    totals: { distance_m: 2224, ascent_m: 0, seconds: 2, surface_m: [0, 0, 0, 0, 0, 0], unknown_elevation_m: 0, pushing_m: 0 },
    legs: [{ from_index: 0, to_index: 2, start: 'a', end: 'b', totals: { distance_m: 2224, ascent_m: 0, seconds: 2, surface_m: [0, 0, 0, 0, 0, 0], unknown_elevation_m: 0, pushing_m: 0 } }],
    snap_truncated: false,
};
const legs = { route: async () => road } as unknown as LegCache;
const point = (id: string, kind: RoutePoint['kind'], coordinate: [number, number], leg?: RoutePoint['leg']): RoutePoint =>
    ({ id, kind, coordinate, label: id, leg });

async function gaps(points: RoutePoint[], loop?: boolean) {
    const plan: Trip = { ...emptyTrip(), bike: 'hiking', points, loop, routeOrder: points.slice(1, loop ? undefined : -1).map(point => point.id) };
    const line = await calculateLine(plan, new AbortController().signal, legs);
    return routeGaps(planView(plan, line).stops, line.coordinates);
}

describe('route gaps', () => {
    it('joins a point far from any road to where the route ends, and keeps the point', async () => {
        // The start is 20 m off the road: a coarse click. The finish is 0.002° (222 m) north of the route end.
        const [gap, ...others] = await gaps([point('s', 'start', [0, 0.00018]), point('f', 'finish', [0.02, 0.002])]);
        expect(others).toEqual([]);
        expect(gap.point.id).toBe('f');
        expect(gap.coordinates).toEqual([[0.02, 0], [0.02, 0.002]]);
        expect(gap.at).toBeCloseTo(2.224, 2);
        expect(gapLabel(gap)).toBe('Ends 220 m before your point');
        expect(gapLabel({ ...gap, point: { ...gap.point, kind: 'start' }, km: 1.24 })).toBe('Starts 1.2 km from your point');
    });

    it('names an off-road loop start once, where the route starts', async () => {
        const [gap, ...others] = await gaps([point('s', 'start', [0, 0.002]), point('v', 'via', [0.02, 0])], true);
        expect(others).toEqual([]);
        expect([gap.point.id, gap.at]).toEqual(['s', 0]);
    });

    it('leaves the join to a manual leg to that leg', async () => {
        const result = await gaps([point('s', 'start', [0, 0]), point('v', 'via', [0.02, 0.002]), point('f', 'finish', [0.03, 0.002], 'straight')]);
        expect(result).toEqual([]);
    });
});
