import { describe, expect, it } from 'vitest';
import { emptyTrip, type DrawnCoordinate, type RoutePoint, type Trip } from './editor';
import { routeFacts } from './route-facts';
import { LegCache } from './route-legs';
import { calculateLine } from './routing';

/** A point at `lon` on the parallel 47, at the end of a drawn leg that climbs or falls from height `from` to `to`. */
function point(id: string, kind: RoutePoint['kind'], lon: number, from: number, to: number, extra: Partial<RoutePoint> = {}): RoutePoint {
    const drawn: DrawnCoordinate[] = [[lon - .05, 47, from], [lon - .025, 47, (from + to) / 2], [lon, 47, to]];
    return { id, kind, label: id, coordinate: [lon, 47], ...kind !== 'start' && { leg: 'drawn', drawn }, ...extra };
}

describe('routeFacts', () => {
    it('gives the days with their waypoints, and the climbs by the rule of the device, of a drawn trip of two days', async () => {
        const trip: Trip = { ...emptyTrip('trip'), days: 2, daysSet: true, routeOrder: ['up', 'night', 'flat'], points: [
            point('start', 'start', 8, 400, 400),
            point('up', 'waypoint', 8.05, 400, 600),
            point('night', 'night', 8.1, 600, 600, { night: 1 }),
            point('flat', 'waypoint', 8.15, 600, 600),
            point('finish', 'finish', 8.2, 600, 400),
        ] };
        const line = await calculateLine(trip, new AbortController().signal, new LegCache());
        const facts = routeFacts(trip, line);

        expect(facts.stops.map(stop => [stop.point.id, stop.number])).toEqual([['up', 1], ['flat', 2]]);
        expect(facts.stops[1].km).toBeCloseTo(11.4, 1);
        expect(facts.days.map(day => [day.number, day.stops.map(stop => stop.number)])).toEqual([[1, [1]], [2, [2]]]);
        expect(facts.days[0].distance).toBeCloseTo(7.6, 1);
        expect(facts.climbs).toHaveLength(1);
        expect(facts.climbs[0]).toMatchObject({ from: 0, ascent: 200 });
        expect(facts.climbs[0].to).toBeCloseTo(3.8, 1);
        expect(facts.climbs[0].top[0]).toBeCloseTo(8.05, 3);
    });
});
