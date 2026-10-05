import { describe, expect, it } from 'vitest';
import { emptyTrip, setEndpoint, addRestDay, applyBudget, maxRidingDays, pinNight, reorderPoint, routingKey, setDrawnLeg } from './editor';
import { isTrip, storedTrip } from './trip-validation';
import { newVersion } from './versions';
import { testTrip } from '../../../test-support/planner/trip';

describe('stored planner data', () => {
    it('round-trips a trip with ordered nights, drawings and rest days at the riding-day limit', () => {
        const plan = applyBudget(testTrip(), 'days', maxRidingDays, 50);
        const first = pinNight(plan, 1, [7.2, 47.5], 'First night');
        const pinned = pinNight(first, 2, [7, 47.5], 'Second night');
        const drawn = setDrawnLeg(pinned, 'night-1', [[7.3, 47.6]]);
        const noted = { ...drawn, points: drawn.points.map(p => p.id === 'night-2' ? { ...p, note: 'Hut, cash only' } : p) };
        const trip = addRestDay(reorderPoint(noted, 'night-2', 1), 2);
        expect(isTrip(JSON.parse(JSON.stringify(trip)))).toBe(true);
        expect(newVersion(trip, 'Trip').trip.points.find(p => p.id === 'night-2')?.note).toBe('Hut, cash only');
        expect(newVersion(trip, 'Trip').trip).toEqual(trip);
    });

    it('drops a stored routing line without edges from a draft and a version', () => {
        const trip = testTrip();
        const stale = { ...trip, routing: { key: routingKey(trip), coordinates: trip.points.map(p => p.coordinate), surfaces: ['Paved'] } };
        expect(storedTrip(stale)).toEqual(trip);

    });

    it('rejects invalid endpoint roles, invalid geometry and broken route references', () => {
        const trip = testTrip();
        const invalid: unknown[] = [
            null, {}, { points: [], days: 3, limit: 50 },
            { ...trip, points: [trip.points[0], trip.points[0]] },
            { ...trip, points: [null, trip.points[1]] },
            { ...trip, points: [{ ...trip.points[0], coordinate: ['7', 47] }, trip.points[1]] },
            { ...trip, points: [{ ...trip.points[0], note: 7 }, trip.points[1]] },
            { ...trip, days: maxRidingDays + 1 }, { ...trip, days: 1.5 },
            { ...trip, budget: 'weeks' }, { ...trip, target: 0 }, { ...trip, bike: 'unknown' },
            { ...trip, routeOrder: undefined }, { ...trip, routeOrder: ['missing'] }, { ...trip, routeOrder: ['finish'] },
            { ...trip, points: [...trip.points, { ...trip.points[0], id: 'shape', kind: 'via' }] },
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
        expect(isTrip({ ...emptyTrip(), points: [{ ...testTrip().points[0], kind: 'via' }] })).toBe(false);
    });

});
