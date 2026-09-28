import { describe, expect, it } from 'vitest';
import { addRestDay, coordinateAt, cumulative, initialTrip, pinNight, routeCoordinates, routeSlice, TripHistory, type Trip } from '../editor';
import { applyQueryChanges, type RouteBuilder } from './actions';

const length = (trip: Trip) => cumulative(routeCoordinates(trip)).at(-1)!;
const direct: RouteBuilder = async points => points.slice(1).map((p,i) => [points[i].coordinate, p.coordinate]);

describe('query edits', () => {
    it('commits a sentence atomically and supports the existing undo history', async () => {
        const before = initialTrip(), saved = structuredClone(before);
        const next = await applyQueryChanges(before, [{ op: 'split', range: [0,length(before)], count: 5 }]);
        expect(next.days).toBe(5);
        expect(routeCoordinates(next)).toEqual(routeCoordinates(before));
        const history = new TripHistory();
        expect(history.undo(history.commit(before,next))).toEqual(before);
        await expect(applyQueryChanges(before, [{ op: 'split', range: [0,length(before)], count: 5 }, { op:'remove_point', id:'absent' }])).rejects.toThrow('changed');
        expect(before).toEqual(saved);
    });
    it('resolves calendar days around rest days and moves a boundary without changing the line', async () => {
        const trip = addRestDay(initialTrip(),1), km = length(trip)*.7;
        const next = await applyQueryChanges(trip, [{ op:'end_day', day:3, point:{coordinate:coordinateAt(routeCoordinates(trip),.7),label:'km mark',along:km} }]);
        expect(next.splits?.[2]).toBeCloseTo(.7);
        expect(routeCoordinates(next)).toEqual(routeCoordinates(trip));
        await expect(applyQueryChanges(trip, [{op:'end_day',day:2,point:{coordinate:[8,48],label:'Inn'}}])).rejects.toThrow('riding day');
    });
    it('reverses a drawn route, overnight labels, and provisional boundaries', async () => {
        const base = initialTrip();
        const trip = pinNight({...base,splits:{2:.7}},1,coordinateAt(routeCoordinates(base),.3),'Inn');
        const next = await applyQueryChanges(trip,[{op:'reverse'}]);
        expect(routeCoordinates(next)).toEqual(routeCoordinates(trip).reverse());
        expect(next.points.find(p=>p.label==='Inn')?.night).toBe(2);
        expect(next.splits?.[1]).toBeCloseTo(.3);
    });
    it('preserves point identities and day commitments on reroute', async () => {
        const base = initialTrip();
        const trip = addRestDay(pinNight(base,1,coordinateAt(routeCoordinates(base),.3),'Inn'),1);
        const next = await applyQueryChanges(trip,[{op:'reroute',range:[0,length(trip)]}],direct);
        expect(new Set(next.points.map(p=>p.id))).toEqual(new Set(trip.points.map(p=>p.id)));
        expect(next.points.find(p=>p.label==='Inn')?.kind).toBe('night');
        expect(next.days).toBe(trip.days); expect(next.restAfter).toEqual(trip.restAfter);
        await expect(applyQueryChanges(base,[{op:'route',points:[{coordinate:[8,48],label:'A'},{coordinate:[8.1,48],label:'B'}]}])).rejects.toThrow('routing engine');
    });
    it('re-routes a selected interval while retaining its outside geometry', async () => {
        const trip = initialTrip(), line = routeCoordinates(trip), total = length(trip);
        const next = await applyQueryChanges(trip,[{op:'reroute',range:[total*.3,total*.6]}],direct);
        const result = routeCoordinates(next);
        const before = routeSlice(line,0,.3), after = routeSlice(line,.6,1);
        expect(result.slice(0,before.length)).toEqual(before);
        expect(result.slice(-after.length)).toEqual(after);
        expect(next.points.map(p=>p.id)).toEqual(trip.points.map(p=>p.id));
    });
});
