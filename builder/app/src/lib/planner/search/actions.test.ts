import { describe, expect, it, vi } from 'vitest';
import { addRestDay, emptyTrip, initialTrip, pinNight, planView, routingKey, TripHistory, type Trip } from '../editor';
import { coordinateAt, routeSlice } from '../geo';
import { calculateLine } from '../routing';
import { LegCache } from '../route-legs';
import { applyQueryChanges, type RouteBuilder } from './actions';

const length = (trip: Trip) => planView(trip).total;
const direct: RouteBuilder = async points => points.slice(1).map((p,i) => [points[i].coordinate, p.coordinate]);

describe('query edits', () => {
    it('creates a route from an empty plan and explains edits that need a route', async () => {
        const empty = emptyTrip();
        const points = [{coordinate:[8,48] as [number,number],label:'A'},{coordinate:[8.1,48] as [number,number],label:'B'}];
        await expect(applyQueryChanges(empty, [{op:'add_point',point:points[0]}])).rejects.toThrow('Choose a start and finish');
        const next = await applyQueryChanges(empty, [{op:'route',points}], direct, trip => calculateLine(trip, new AbortController().signal, new LegCache()));
        expect(next.points.map(p => p.kind)).toEqual(['start','finish']);
        expect(planView(next).coordinates).toEqual(points.map(p => p.coordinate));
        expect(empty.points).toEqual([]);
    });
    it('commits a sentence atomically and supports the existing undo history', async () => {
        const before = initialTrip(), saved = structuredClone(before);
        const next = await applyQueryChanges(before, [{ op: 'split', range: [0,length(before)], count: 5 }]);
        expect(next.days).toBe(5);
        expect(planView(next).coordinates).toEqual(planView(before).coordinates);
        const history = new TripHistory();
        expect(history.undo(history.commit(before,next))).toEqual(before);
        await expect(applyQueryChanges(before, [{ op: 'split', range: [0,length(before)], count: 5 }, { op:'remove_point', id:'absent' }])).rejects.toThrow('changed');
        expect(before).toEqual(saved);
    });
    it('resolves calendar days around rest days and moves a boundary without changing the line', async () => {
        const trip = addRestDay(initialTrip(),1), km = length(trip)*.7;
        const next = await applyQueryChanges(trip, [{ op:'end_day', day:3, point:{coordinate:coordinateAt(planView(trip).coordinates,.7),label:'km mark',along:km} }]);
        expect(next.splits?.[2]).toBeCloseTo(.7);
        expect(planView(next).coordinates).toEqual(planView(trip).coordinates);
        await expect(applyQueryChanges(trip, [{op:'end_day',day:2,point:{coordinate:[8,48],label:'Inn'}}])).rejects.toThrow('riding day');
    });
    it('reverses a drawn route, overnight labels, and provisional boundaries', async () => {
        const base = initialTrip();
        const trip = pinNight({...base,splits:{2:.7}},1,coordinateAt(planView(base).coordinates,.3),'Inn');
        const next = await applyQueryChanges(trip,[{op:'reverse'}]);
        expect(planView(next).coordinates).toEqual([...planView(trip).coordinates].reverse());
        expect(next.points.find(p=>p.label==='Inn')?.night).toBe(2);
        expect(next.splits?.[1]).toBeCloseTo(.3);
    });
    it('reverses a loop and keeps its start', async () => {
        const point = (id: string, kind: 'start' | 'waypoint', coordinate: [number, number]) => ({ id, kind, label: id, coordinate, progress: 0, leg: 'straight' as const });
        const plan: Trip = { ...emptyTrip(), loop: true, routeOrder: ['a', 'b'], points: [point('home', 'start', [8, 48]), point('a', 'waypoint', [8.1, 48]), point('b', 'waypoint', [8.1, 48.1])] };
        const refresh = (next: Trip) => calculateLine(next, new AbortController().signal, new LegCache());
        const trip = { ...plan, routing: await refresh(plan) };
        const next = await applyQueryChanges(trip, [{op:'reverse'}], undefined, refresh);
        expect(next.loop).toBe(true);
        expect(next.points.find(p => p.kind === 'start')?.id).toBe('home');
        expect(planView(next).coordinates).toEqual([...planView(trip).coordinates].reverse());
    });
    it('preserves point identities and day commitments on reroute', async () => {
        const base = initialTrip();
        const trip = addRestDay(pinNight(base,1,coordinateAt(planView(base).coordinates,.3),'Inn'),1);
        const next = await applyQueryChanges(trip,[{op:'reroute',range:[0,length(trip)]}],direct);
        expect(new Set(next.points.map(p=>p.id))).toEqual(new Set(trip.points.map(p=>p.id)));
        expect(next.points.find(p=>p.label==='Inn')?.kind).toBe('night');
        expect(next.days).toBe(trip.days); expect(next.restAfter).toEqual(trip.restAfter);
        await expect(applyQueryChanges(base,[{op:'route',points:[{coordinate:[8,48],label:'A'},{coordinate:[8.1,48],label:'B'}]}])).rejects.toThrow('routing engine');
    });
    it('refreshes live geometry between edits and preserves the original on a routing failure', async () => {
        const base = initialTrip(), line = planView(base).coordinates;
        const plan: Trip = {...base, live:true, routeOrder:[], points:[base.points[0], {...base.points.at(-1)!,leg:'drawn',drawn:line.slice(1,-1)}]};
        const trip = { ...plan, routing: await calculateLine(plan, new AbortController().signal, new LegCache()) };
        const before = structuredClone(trip);
        const refresh = vi.fn((next: Trip) => calculateLine(next, new AbortController().signal, new LegCache()));
        const next = await applyQueryChanges(trip, [{op:'reverse'}, {op:'reverse'}], undefined, refresh);
        expect(refresh).toHaveBeenCalledTimes(2);
        expect(next.routing?.key).toBe(routingKey(next));
        expect(planView(next).coordinates).toEqual(line);
        const failing = vi.fn().mockImplementationOnce(refresh).mockRejectedValue(new Error('Routing unavailable'));
        await expect(applyQueryChanges(trip, [{op:'reverse'}, {op:'reverse'}], undefined, failing)).rejects.toThrow('Routing unavailable');
        expect(trip).toEqual(before);
    });
    it('re-routes a selected interval while retaining its outside geometry', async () => {
        const trip = initialTrip(), line = planView(trip).coordinates, total = length(trip);
        const next = await applyQueryChanges(trip,[{op:'reroute',range:[total*.3,total*.6]}],direct);
        const result = planView(next).coordinates;
        const before = routeSlice(line,0,.3), after = routeSlice(line,.6,1);
        expect(result.slice(0,before.length)).toEqual(before);
        expect(result.slice(-after.length)).toEqual(after);
        expect(next.points.map(p=>p.id)).toEqual(trip.points.map(p=>p.id));
    });
});
