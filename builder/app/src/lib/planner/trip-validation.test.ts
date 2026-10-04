import { describe, expect, it } from 'vitest';
import { emptyTrip, setEndpoint, addRestDay, applyBudget, initialTrip, maxRidingDays, pinNight, reorderPoint, routingKey, setDrawnLeg } from './editor';
import { isTrip, storedTrip } from './trip-validation';
import { newVersion } from './versions';

describe('stored planner data', () => {
    it('round-trips a trip with ordered nights, drawings and rest days at the riding-day limit', () => {
        const plan = applyBudget(initialTrip(), 'days', maxRidingDays, 50);
        const first = pinNight(plan, 1, [7.2, 47.5], 'First night');
        const pinned = pinNight(first, 2, [7, 47.5], 'Second night');
        const drawn = setDrawnLeg(pinned, 'night-1', [[7.3, 47.6]]);
        const trip = addRestDay(reorderPoint(drawn, 'night-1', 1), 2);
        expect(isTrip(JSON.parse(JSON.stringify(trip)))).toBe(true);
        expect(newVersion(trip, 'Trip').trip).toEqual(trip);
    });

    it('drops a stored routing line without edges from a draft and a version', () => {
        const trip = initialTrip();
        const stale = { ...trip, routing: { key: routingKey(trip), coordinates: trip.points.map(p => p.coordinate), surfaces: ['Paved'] } };
        expect(storedTrip(stale)).toEqual(trip);

    });

    it('rejects invalid endpoint roles, invalid geometry and broken route references', () => {
        const trip = initialTrip();
        const invalid: unknown[] = [
            null, {}, { points: [], days: 3, limit: 50 },
            { ...trip, points: [trip.points[0], trip.points[0]] },
            { ...trip, points: [null, trip.points[1]] },
            { ...trip, points: [{ ...trip.points[0], coordinate: ['7', 47] }, trip.points[1]] },
            { ...trip, points: [{ ...trip.points[0], progress: null }, trip.points[1]] },
            { ...trip, days: maxRidingDays + 1 }, { ...trip, days: 1.5 },
            { ...trip, budget: 'weeks' }, { ...trip, target: 0 }, { ...trip, bike: 'unknown' },
            { ...trip, routeOrder: ['missing'] }, { ...trip, routeOrder: ['finish', 'finish'] },
            { ...trip, restAfter: [4] }, { ...trip, splits: { 3: .5 } },
            { ...trip, points: [...trip.points, { ...trip.points[0], id: 'night-3', kind: 'night', night: 3 }] },
            { ...trip, points: [trip.points[0], { ...trip.points[1], leg: 'ferry' }] },
        ];
        expect(isTrip({ ...trip, points: [trip.points[0], { ...trip.points[1], leg: 'transfer' }] })).toBe(true);
        for (const value of invalid) expect(isTrip(value), JSON.stringify(value)).toBe(false);
    });

    it('round-trips empty and one-endpoint drafts and versions', () => {
        for (const trip of [emptyTrip(), setEndpoint(emptyTrip('trip'), 'finish', [8, 48], 'Finish')]) {
            expect(isTrip(JSON.parse(JSON.stringify(trip)))).toBe(true);
            expect(newVersion(trip).trip).toEqual(trip);
        }
        expect(isTrip({ ...emptyTrip(), points: [{ ...initialTrip().points[0], kind: 'via' }] })).toBe(false);
    });

});
