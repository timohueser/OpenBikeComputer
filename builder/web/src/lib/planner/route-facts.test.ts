import { describe, expect, it } from 'vitest';
import { emptyTrip, type DrawnCoordinate, type RoutePoint, type Trip } from './editor';
import { routeFacts, routePins } from './route-facts';
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

    it('gives a climb the grade of its steepest 500 m, which a spike of one height does not raise', async () => {
        // 50 m over 3.8 km, then a ramp of 300 m over 3.8 km (7.9 %) with a spike of 8 m over 100 m in its middle: one
        // climb with a mean grade of 4.6 %. A 100 m window on the spike gives 12 %; the 500 m window at most 9.5 %.
        const finish = point('finish', 'finish', 8.1, 450, 750);
        const trip: Trip = { ...emptyTrip('route'), days: 1, routeOrder: ['foot'], points: [
            point('start', 'start', 8, 400, 400),
            point('foot', 'waypoint', 8.05, 400, 450),
            { ...finish, drawn: [[8.05, 47, 450], [8.075, 47, 600], [8.07566, 47, 612], [8.07632, 47, 608], [8.1, 47, 750]] },
        ] };
        const facts = routeFacts(trip, await calculateLine(trip, new AbortController().signal, new LegCache()));

        expect(facts.climbs).toHaveLength(1);
        expect(facts.climbs[0].gradient).toBeCloseTo(4.6, 1);
        expect(facts.climbs[0].steepest).toBeGreaterThan(7.8);
        expect(facts.climbs[0].steepest).toBeLessThan(10);
    });

    it('places the waypoints and the nights on the calculated line, and with no line at the nearest points of the summary line', async () => {
        const trip: Trip = { ...emptyTrip('trip'), days: 2, daysSet: true, routeOrder: ['up', 'night-1', 'flat'], points: [
            point('start', 'start', 8, 400, 400),
            point('up', 'waypoint', 8.05, 400, 600),
            point('night-1', 'night', 8.1, 600, 600, { night: 1 }),
            point('flat', 'waypoint', 8.15, 600, 600),
            point('finish', 'finish', 8.2, 600, 400),
        ] };
        const line = await calculateLine(trip, new AbortController().signal, new LegCache());
        const summary = [8, 8.05, 8.1, 8.15, 8.2].map((lon): [number, number] => [lon, 47]);

        for (const pins of [routePins(trip, summary, line), routePins(trip, summary)]) {
            expect(pins.map(pin => [pin.label, pin.number])).toEqual([['up', 1], ['flat', 2], ['night-1', undefined]]);
            expect(pins.map(pin => pin.share)).toEqual([expect.closeTo(.25, 2), expect.closeTo(.75, 2), expect.closeTo(.5, 2)]);
        }
    });
});
