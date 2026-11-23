import { afterEach, describe, expect, it, vi } from 'vitest';
import { addRestDay, emptyTrip, loopTrip, maxRidingDays, pinNight, planView, removeRoutePoint, type RoutePoint, type Trip } from '../editor';
import { isTrip } from '../trip-validation';
import { coordinateAt, type Coordinate } from '../geo';
import { testLine, testTrip } from '../../../../test-support/planner/trip';
import { calculateLine, type RoutingLine } from '../routing';
import { LegCache } from '../route-legs';
import { routeService } from '../../../../test-support/planner/route-service';
import { applyQueryChanges, placeChange, placeCheck } from './actions';
import type { QueryChange, SearchContext, SearchPlace } from './types';

const view = ({ trip, line }: { trip: Trip; line?: RoutingLine }) => planView(trip, line);
const refresh = (trip: Trip) => calculateLine(trip, new AbortController().signal, new LegCache());
afterEach(() => vi.unstubAllGlobals());

describe('query edits', () => {
    it('creates routed plan points with the requested bike, preset and day count', async () => {
        const fetch = vi.fn(routeService());
        vi.stubGlobal('fetch', fetch);
        const empty = emptyTrip();
        const points = [{coordinate:[8,48] as Coordinate,label:'A'},{coordinate:[8.05,48] as Coordinate,label:'B',kind:'town'},{coordinate:[8.1,48] as Coordinate,label:'C'}];
        await expect(applyQueryChanges(empty, undefined, [{op:'add_point',point:points[0]}])).rejects.toThrow('Choose a start and finish');
        await expect(applyQueryChanges(empty, undefined, [{op:'route',points}])).rejects.toThrow('routing engine');
        const { trip: next, line } = await applyQueryChanges(empty, undefined, [{op:'route',points,bike:'gravel',goal:'least_climbing',perDay:{value:3,unit:'km'}}], refresh);
        expect(next.points.map(p => [p.kind, p.label, p.leg])).toEqual([['start','A',undefined],['pass','B',undefined],['finish','C',undefined]]);
        expect([next.bike, next.preset, next.days, next.daysSet, next.mode]).toEqual(['gravel', 'Less climbing', 3, true, 'trip']);
        expect(JSON.parse(fetch.mock.calls[0][1].body as string)).toMatchObject({ points: points.map(p => p.coordinate), profile: 'gravel/less-climbing' });
        expect(line?.stops.map(stop => stop.id)).toEqual(next.points.map(p => p.id));
        expect(line?.unroutedKm).toBe(0);
        expect(empty.points).toEqual([]);
        await expect(applyQueryChanges(empty, undefined, [{ op: 'route', points, days: maxRidingDays + 1 }], refresh)).rejects.toThrow('riding days');
    });
    it('keeps the rider preset without a goal and drops the name of the replaced route', async () => {
        vi.stubGlobal('fetch', vi.fn(routeService()));
        const signed: Trip = { ...emptyTrip(), name: 'Westweg', bike: 'mtb', preset: 'Technical', nightStay: { 1: ['hut'] } };
        const here = {coordinate:[8,48] as Coordinate,label:'Here'};
        const { trip: next } = await applyQueryChanges(signed, undefined, [{op:'route',points:[here,{coordinate:[8.1,48],label:'Titisee'}],days:3}], refresh);
        expect([next.name, next.bike, next.preset, next.nightStay]).toEqual([undefined, 'mtb', 'Technical', undefined]);
        const { trip: same } = await applyQueryChanges(signed, undefined, [{op:'route',points:[here,here],perDay:{value:80,unit:'km'}}], refresh);
        expect(same.days).toBe(1);
        expect((await applyQueryChanges({ ...signed, preset: 'Less climbing' }, undefined, [{op:'route',points:[here,here],bike:'gravel'}], refresh)).trip.preset).toBe('Balanced');
        const { trip: untyped } = await applyQueryChanges({ ...signed, daysSet: true }, undefined, [{op:'route',points:[here,{coordinate:[8.1,48],label:'Titisee'}]}], refresh);
        expect(untyped.daysSet).toBeUndefined();
    });
    it('rejects an edit it does not know instead of committing an unchanged plan', async () => {
        const reroute = { op: 'reroute', range: [0, 10] } as unknown as QueryChange;
        await expect(applyQueryChanges(testTrip(), undefined, [reroute])).rejects.toThrow('not available');
    });
    it('commits a sentence atomically and keeps the line of an itinerary edit', async () => {
        const before = { trip: testTrip(), line: testLine(testTrip()) }, saved = structuredClone(before.trip);
        const total = view(before).total;
        const next = await applyQueryChanges(before.trip, before.line, [{ op: 'split', range: [0,total], count: 5 }]);
        expect(next.trip).toMatchObject({ days: 5, daysSet: true });
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
    it('ends a day at a kilometre instead of its pinned night, which leaves the route', async () => {
        vi.stubGlobal('fetch', vi.fn(routeService()));
        const pinned = pinNight(testTrip(), testLine(testTrip()), 1, coordinateAt(testLine(testTrip()).coordinates, .4), 'Camp');
        const line = await refresh(pinned), km = view({ trip: pinned, line }).total * .3;
        const next = await applyQueryChanges(pinned, line, [{ op:'end_day', day:1, point:{coordinate:coordinateAt(line.coordinates,.3),label:'km mark',along:km} }], refresh);
        expect([next.trip.points.map(p => p.id), next.trip.routeOrder, isTrip(next.trip)]).toEqual([['start', 'finish'], [], true]);
        expect(next.trip.splits?.[1]).toBeCloseTo(.3);
    });
    it('uses shared removal rules and leaves a collapsed loop unrouted', async () => {
        const trip = { ...loopTrip(emptyTrip(), [[8, 48], [8.1, 48]], 'Home'), splits: { 1: .4 } };
        const before = structuredClone(trip), id = trip.routeOrder[0];
        const route = vi.fn();
        const next = await applyQueryChanges(trip, testLine(trip), [{ op: 'remove_point', id }], route);
        expect(next.trip).toEqual(removeRoutePoint(trip, id));
        expect(next.trip.loop).toBeUndefined();
        expect(next.trip.splits).toBeUndefined();
        expect(isTrip(next.trip)).toBe(true);
        expect(next.line).toBeUndefined();
        expect(route).not.toHaveBeenCalled();
        await expect(applyQueryChanges(trip, testLine(trip), [{ op: 'remove_point', id }, { op: 'reverse' }], route))
            .rejects.toThrow('Choose a start and finish');
        expect(trip).toEqual(before);
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

describe('the action on a place of the search list', () => {
    // A riding day, a rest day and a riding day on a line of 15 km.
    const plan: SearchContext['plan'] = { coordinates: [[8, 48], [8.1, 48], [8.2, 48]], km: [0, 7.45, 14.9],
        points: [{ id: 'start', label: 'Freiburg', coordinate: [8, 48], kind: 'start' }, { id: 'finish', label: 'Konstanz', coordinate: [8.2, 48], kind: 'finish' }],
        days: [{ number: 1, from: 0, to: 7.45, rest: false }, { number: 2, from: 7.45, to: 7.45, rest: true }, { number: 3, from: 7.45, to: 14.9, rest: false }] };
    const place = (position?: SearchPlace['position']): SearchPlace => ({ source: 'n/1', name: 'Rewe', kind: 'supermarket', lon: 8.15, lat: 48.001, city: '', region: '', distance: 0, precision: 'place', position });
    const point = { coordinate: [8.15, 48.001], label: 'Rewe', source: 'n/1', kind: 'supermarket' };
    it('routes from the rider, else from the start, and puts a stop and a day end in the riding day of the place', () => {
        expect(placeChange('route', place(), plan, [7.9, 48])).toEqual({ changes: [{ op: 'route', points: [{ coordinate: [7.9, 48], label: 'Your location' }, point] }], description: 'The route now ends at Rewe' });
        expect(placeChange('route', place(), plan).changes[0].points?.[0]).toEqual({ coordinate: [8, 48], label: 'Freiburg' });
        expect(() => placeChange('route', place(), { ...plan, points: [] })).toThrow('allow location access');
        expect(placeChange('add_point', place({ along: 3, distance: 0 }), plan)).toEqual({ changes: [{ op: 'add_point', kind: 'stop', point }], description: 'Rewe is now a stop on Day 1' });
        // With no place along the route, the nearest point of the route gives the day.
        expect(placeChange('end_day', place(), plan)).toEqual({ changes: [{ op: 'end_day', day: 3, point }], description: 'Day 3 now ends at Rewe' });
    });
    it('applies a day end only before the last riding day, a stop or a day end only with a start and a finish or a loop, and names a point of the plan', () => {
        const trip = (points: Trip['points'], loop?: boolean): Trip => ({ ...emptyTrip(), points, ...loop ? { loop: true } : {} });
        const start = { id: 'start', kind: 'start' as const, label: 'Freiburg', coordinate: [8, 48] as Coordinate };
        const finish = { ...start, id: 'finish', kind: 'finish' as const, label: 'Konstanz', coordinate: [8.2, 48] as Coordinate };
        const at = (kind: RoutePoint['kind']) => trip([start, { ...start, id: 'p', kind, label: 'Rewe', placeId: 'n/1' }, finish]);
        const late = place({ along: 9, distance: 0 });
        expect(placeCheck('end_day', place({ along: 3, distance: 0 }), plan, trip([start, finish]))).toEqual({});
        expect(placeCheck('end_day', late, plan, trip([start, finish]))).toEqual({ reason: 'The last day ends at the finish.' });
        expect([placeCheck('add_point', late, plan, at('waypoint')), placeCheck('add_point', late, plan, at('night')), placeCheck('end_day', late, plan, at('waypoint'))])
            .toEqual([{ point: 'Stop on Day 3' }, { point: 'Night of Day 3' }, { reason: 'The last day ends at the finish.' }]);
        // A loop has a start and no finish.
        expect(placeCheck('add_point', late, plan, trip([start], true))).toEqual({});
        expect(placeCheck('add_point', place(), plan, trip([start]))).toEqual({ reason: 'Choose a start and finish first.' });
        expect(placeCheck('route', place(), plan, trip([]))).toEqual({});
    });
});
