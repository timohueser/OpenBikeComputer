import { afterEach, describe, expect, it, vi } from 'vitest';
import { addRestDay, emptyTrip, planView, type RoutePoint, type Trip } from '../editor';
import { coordinateAt, type Coordinate } from '../geo';
import { testLine, testTrip } from '../../../../test-support/planner/trip';
import { calculateLine, type RoutingLine } from '../routing';
import { LegCache } from '../route-legs';
import { routeService } from '../../../../test-support/planner/route-service';
import { applyQueryChanges } from './actions';
import type { QueryChange } from './types';

const view = ({ trip, line }: { trip: Trip; line?: RoutingLine }) => planView(trip, line);
const refresh = (trip: Trip) => calculateLine(trip, new AbortController().signal, new LegCache());
const point = (id: string, kind: RoutePoint['kind'], coordinate: Coordinate, extra: Partial<RoutePoint> = {}): RoutePoint => ({ id, kind, label: id, coordinate, ...extra });
afterEach(() => vi.unstubAllGlobals());

describe('query edits', () => {
    it('creates routed plan points with the requested bike, preset and day budget', async () => {
        const fetch = vi.fn(routeService());
        vi.stubGlobal('fetch', fetch);
        const empty = emptyTrip();
        const points = [{coordinate:[8,48] as Coordinate,label:'A'},{coordinate:[8.05,48] as Coordinate,label:'B',kind:'town'},{coordinate:[8.1,48] as Coordinate,label:'C'}];
        await expect(applyQueryChanges(empty, undefined, [{op:'add_point',point:points[0]}])).rejects.toThrow('Choose a start and finish');
        await expect(applyQueryChanges(empty, undefined, [{op:'route',points}])).rejects.toThrow('routing engine');
        const { trip: next, line } = await applyQueryChanges(empty, undefined, [{op:'route',points,bike:'gravel',goal:'least_climbing',perDay:{value:3,unit:'km'}}], refresh);
        expect(next.points.map(p => [p.kind, p.label, p.leg])).toEqual([['start','A',undefined],['pass','B',undefined],['finish','C',undefined]]);
        expect([next.bike, next.preset, next.days, next.mode]).toEqual(['gravel', 'Less climbing', 3, 'trip']);
        expect(JSON.parse(fetch.mock.calls[0][1].body as string)).toMatchObject({ points: points.map(p => p.coordinate), profile: 'gravel/less-climbing' });
        expect(line?.stops.map(stop => stop.id)).toEqual(next.points.map(p => p.id));
        expect(line?.unroutedKm).toBe(0);
        expect(empty.points).toEqual([]);
    });
    it('keeps the rider preset without a goal and drops the name of the replaced route', async () => {
        vi.stubGlobal('fetch', vi.fn(routeService()));
        const signed: Trip = { ...emptyTrip(), name: 'Westweg', bike: 'mtb', preset: 'Less climbing' };
        const here = {coordinate:[8,48] as Coordinate,label:'Here'};
        const { trip: next } = await applyQueryChanges(signed, undefined, [{op:'route',points:[here,{coordinate:[8.1,48],label:'Titisee'}]}], refresh);
        expect([next.name, next.bike, next.preset]).toEqual([undefined, 'mtb', 'Less climbing']);
        const { trip: same } = await applyQueryChanges(signed, undefined, [{op:'route',points:[here,here],perDay:{value:80,unit:'km'}}], refresh);
        expect([same.days, same.target]).toEqual([1, 1]);
    });
    it('rejects an edit it does not know instead of committing an unchanged plan', async () => {
        const reroute = { op: 'reroute', range: [0, 10] } as unknown as QueryChange;
        await expect(applyQueryChanges(testTrip(), undefined, [reroute])).rejects.toThrow('not available');
    });
    it('commits a sentence atomically and keeps the line of an itinerary edit', async () => {
        const before = { trip: testTrip(), line: testLine(testTrip()) }, saved = structuredClone(before.trip);
        const total = view(before).total;
        const next = await applyQueryChanges(before.trip, before.line, [{ op: 'split', range: [0,total], count: 5 }]);
        expect(next.trip.days).toBe(5);
        expect(next.line).toBe(before.line);
        await expect(applyQueryChanges(before.trip, before.line, [{ op: 'split', range: [0,total], count: 5 }, { op:'remove_point', id:'absent' }])).rejects.toThrow('changed');
        expect(before.trip).toEqual(saved);
    });
    it('resolves calendar days around rest days and moves a boundary without changing the line', async () => {
        const trip = addRestDay(testTrip(),1), line = testLine(trip), km = view({ trip, line }).total*.7;
        const next = await applyQueryChanges(trip, line, [{ op:'end_day', day:3, point:{coordinate:coordinateAt(line.coordinates,.7),label:'km mark',along:km} }]);
        expect(next.trip.splits?.[2]).toBeCloseTo(.7);
        expect(next.line).toBe(line);
        await expect(applyQueryChanges(trip, line, [{op:'end_day',day:2,point:{coordinate:[8,48],label:'Inn'}}])).rejects.toThrow('riding day');
    });
    it('reverses the point order, routes routed legs again and keeps drawn legs and nights', async () => {
        const fetch = vi.fn(routeService());
        vi.stubGlobal('fetch', fetch);
        const plan: Trip = { ...emptyTrip('trip'), splits: { 2: .7 }, routeOrder: ['night-1'], points: [
            point('a', 'start', [8, 48]), point('night-1', 'night', [8.1, 48], { night: 1 }),
            point('c', 'finish', [8.2, 48], { leg: 'drawn', drawn: [[8.15, 48.01, 300], [8.12, 48.01, 310]] }),
        ] };
        const line = await refresh(plan);
        const next = await applyQueryChanges(plan, line, [{op:'reverse'}], refresh);
        expect(next.trip.points.map(p => [p.id, p.kind, p.leg ?? 'routed', p.night])).toEqual([['c', 'start', 'routed', undefined], ['night-2', 'night', 'drawn', 2], ['a', 'finish', 'routed', undefined]]);
        expect(next.trip.points[1].drawn).toEqual([[8.12, 48.01, 310], [8.15, 48.01, 300]]);
        expect(JSON.parse(fetch.mock.calls.at(-1)![1].body as string).points).toEqual([[8.1, 48], [8, 48]]);
        expect(view(next).coordinates).toEqual([...line.coordinates].reverse());
        expect(next.trip.splits?.[1]).toBeCloseTo(.3);
    });
    it('reverses a loop and keeps its start, which holds the closing leg', async () => {
        const plan: Trip = { ...emptyTrip(), loop: true, routeOrder: ['a', 'b'], points: [point('home', 'start', [8, 48], { leg: 'straight' }),
            point('a', 'waypoint', [8.1, 48], { leg: 'drawn', drawn: [[8.05, 47.99]] }), point('b', 'waypoint', [8.1, 48.1], { leg: 'straight' })] };
        const line = await refresh(plan);
        const next = await applyQueryChanges(plan, line, [{op:'reverse'}], refresh);
        expect(next.trip.loop).toBe(true);
        expect(next.trip.points.map(p => [p.id, p.kind, p.leg])).toEqual([['home', 'start', 'drawn'], ['b', 'waypoint', 'straight'], ['a', 'waypoint', 'straight']]);
        expect(view(next).coordinates).toEqual([...line.coordinates].reverse());
    });
    it('refreshes live geometry between edits and preserves the original on a routing failure', async () => {
        const base = testTrip(), inner: Coordinate[] = [[7.7, 47.4], [7.5, 47.1]];
        const drawn = [base.points[0].coordinate, ...inner, base.points[1].coordinate];
        const plan: Trip = {...base, points:[base.points[0], {...base.points[1],leg:'drawn',drawn:inner}]};
        const line = await refresh(plan);
        const before = structuredClone(plan);
        const counted = vi.fn(refresh);
        const next = await applyQueryChanges(plan, line, [{op:'reverse'}, {op:'reverse'}], counted);
        expect(counted).toHaveBeenCalledTimes(2);
        expect(next.line).toBe(await counted.mock.results[1].value);
        expect(view(next).coordinates).toEqual(drawn);
        const failing = vi.fn().mockImplementationOnce(refresh).mockRejectedValue(new Error('Routing unavailable'));
        await expect(applyQueryChanges(plan, line, [{op:'reverse'}, {op:'reverse'}], failing)).rejects.toThrow('Routing unavailable');
        expect(plan).toEqual(before);
    });
});
