import { describe, expect, it } from 'vitest';
import { emptyTrip, setEndpoint, addRestDay, applyBudget, initialTrip, maxRidingDays, pinNight, reorderPoint, setDrawnLeg } from './editor';
import { isTrip } from './trip-validation';
import { listVersions, readVersion, saveVersion } from './versions';

describe('stored planner data', () => {
    it('round-trips a trip with ordered nights, drawings and rest days at the riding-day limit', () => {
        const plan = applyBudget(initialTrip(), 'days', maxRidingDays, 50);
        const first = pinNight(plan, 1, [7.2, 47.5], 'First night');
        const pinned = pinNight(first, 2, [7, 47.5], 'Second night');
        const drawn = setDrawnLeg(pinned, 'night-1', [[7.3, 47.6]]);
        const trip = addRestDay(reorderPoint(drawn, 'night-1', 1), 2);
        expect(isTrip(JSON.parse(JSON.stringify(trip)))).toBe(true);
        let json: string | null = null;
        const store = { getItem: () => json, setItem: (_: string, value: string) => { json = value; } };
        const version = saveVersion(trip, 'Trip', store);
        expect(readVersion(version.id, store)).toEqual(trip);
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
        ];
        for (const value of invalid) expect(isTrip(value), JSON.stringify(value)).toBe(false);
    });

    it('round-trips empty and one-endpoint drafts and versions', () => {
        for (const trip of [emptyTrip(), setEndpoint(emptyTrip('trip'), 'finish', [8, 48], 'Finish')]) {
            expect(isTrip(JSON.parse(JSON.stringify(trip)))).toBe(true);
            let json: string | null = null;
            const store = { getItem: () => json, setItem: (_: string, value: string) => { json = value; } };
            const saved = saveVersion(trip, undefined, store);
            expect(readVersion(saved.id, store)).toEqual(trip);
        }
        expect(isTrip({ ...emptyTrip(), points: [{ ...initialTrip().points[0], kind: 'via' }] })).toBe(false);
    });

    it('skips malformed and duplicate version entries while keeping readable versions', () => {
        const valid = { id: 'valid', at: '2026-01-01T00:00:00Z', summary: 'Trip', trip: initialTrip() };
        const store = { getItem: () => JSON.stringify([null, {}, { ...valid, id: 'broken', trip: {} }, valid, valid]), setItem: () => {} };
        expect(listVersions(store)).toEqual([valid]);
        expect(readVersion('broken', store)).toBeUndefined();
        expect(readVersion('valid', store)).toEqual(valid.trip);
    });
});
