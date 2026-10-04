import { readFileSync } from 'node:fs';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { emptyTrip, orderedRoutePoints, type Coordinate } from './editor';
import { LegCache } from './route-legs';
import { decodeCoordinates } from './route-answer';
import { calculateLine } from './routing';
import { joinedPlan, planTrip, recordPlan } from './signed-route-plan';
import { routePlan, type RouteRecord } from './signed-routes';
import { isTrip } from './trip-validation';

const routes: RouteRecord[] = JSON.parse(readFileSync(new URL('../../../../../specs/vectors/signed-routes.json', import.meta.url), 'utf8')).routes;
const record = (id: number) => routes.find(route => route.id === id)!;
const line = (id: number) => decodeCoordinates(record(id).line_udeg);
const base = () => ({ ...emptyTrip(), bike: 'hiking' as const, preset: 'Balanced' });

afterEach(() => vi.unstubAllGlobals());

describe('signed route plans', () => {
    it('starts a loop at a chosen vertex and keeps the shaping points in loop order', () => {
        const loop = line(103);
        // Vertex 3 is a shaping point, so the plan keeps its point count.
        expect(recordPlan(record(103), 3).points).toEqual([3, 6, 9, 0, 3].map(i => loop[i]));
        expect(recordPlan(record(103), 4).points).toEqual([4, 6, 9, 0, 3, 4].map(i => loop[i]));
        expect(recordPlan(record(103), 0)).toEqual(routePlan(record(103)));
        // A one-way route always runs in data order.
        expect(recordPlan(record(102), 2)).toEqual(routePlan(record(102)));
    });

    it('moves the turnarounds of a rotated loop with their points', () => {
        const loop = { ...record(103), turnarounds: [9] };
        expect(recordPlan(loop, 4).turnarounds).toEqual([2]);
        expect(recordPlan(loop, 9).turnarounds).toEqual([]);
    });

    it('joins the stages of a long route at their shared ends', () => {
        const [first, second] = [routePlan(record(402)), routePlan(record(403))];
        expect(joinedPlan([first, second])).toEqual({ points: [...first.points, ...second.points.slice(1)], turnarounds: [] });
        const turning = { ...second, turnarounds: [2] };
        expect(joinedPlan([first, turning])!.turnarounds).toEqual([5]);
    });

    it('offers a long route whole only when the joined plan fits one request', () => {
        const stage = (from: number) => ({ points: Array.from({ length: 22 }, (_, i): Coordinate => [8 + (from + i) / 100, 48]), turnarounds: [] });
        expect(joinedPlan([stage(0), stage(21), stage(42)])!.points).toHaveLength(64);
        expect(joinedPlan([stage(0), stage(21), stage(42), stage(63)])).toBeNull();
    });

    it('builds a loop plan with one start and hidden shaping points', () => {
        const plan = recordPlan(record(103), 4);
        const trip = planTrip(base(), plan, true);
        expect(trip.loop).toBe(true);
        expect(orderedRoutePoints(trip).map(point => point.coordinate)).toEqual(plan.points);
        expect(trip.points.filter(point => point.kind === 'via').every(point => point.hidden)).toBe(true);
        expect(isTrip(trip)).toBe(true);
    });

    it('sends the turnarounds of a one-way plan with its route request', async () => {
        const plan = routePlan(record(106));
        const trip = { ...planTrip(base(), plan, false), live: true };
        expect(orderedRoutePoints(trip).map(point => [point.kind, point.coordinate])).toEqual(
            plan.points.map((coordinate, i) => [i === 0 ? 'start' : i === plan.points.length - 1 ? 'finish' : 'via', coordinate]));
        expect(isTrip(trip)).toBe(true);
        const udeg = plan.points.flatMap(([lon, lat], i) => i ? [Math.round((lon - plan.points[i - 1][0]) * 1e6), Math.round((lat - plan.points[i - 1][1]) * 1e6)]
            : [Math.round(lon * 1e6), Math.round(lat * 1e6)]);
        const totals = { distance_m: 1, ascent_m: 0, seconds: 1, surface_m: [0, 1, 0, 0, 0, 0], unknown_elevation_m: 0, pushing_m: 0 };
        const answer = { id: 'route', reason: 'primary', package: 'test', profile: 'hiking', coordinates_udeg: udeg, elevation_dm: plan.points.map(() => 0),
            elapsed_s: plan.points.map(() => 1), edges: {}, totals, snap_truncated: false,
            legs: plan.points.slice(1).map((_, i) => ({ from_index: i, to_index: i + 1, start: `${i}`, end: `${i + 1}`, totals })) };
        const fetch = vi.fn().mockResolvedValue({ ok: true, json: async () => ({ routes: [answer] }) });
        vi.stubGlobal('fetch', fetch);
        await calculateLine(trip, new AbortController().signal, new LegCache());
        expect(JSON.parse(fetch.mock.calls[0][1].body)).toMatchObject({ points: plan.points, turnarounds: plan.turnarounds });
    });
});
