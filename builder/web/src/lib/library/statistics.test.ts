import { describe, expect, it } from 'vitest';
import type { Bike, RideRecord } from './records';
import { yearTotals } from './statistics';

const ride = (start: Date, lengthM: number, bike: Bike = 'road') => ({ summary: { start: start.getTime() / 1000, lengthM, bike } }) as RideRecord;

describe('yearTotals', () => {
    it('counts each ride in the year of its local start, and a hike apart', () => {
        const rides = [ride(new Date(2025, 11, 31, 23, 30), 40_000), ride(new Date(2026, 0, 1, 0, 30), 30_000),
            ride(new Date(2026, 5, 1), 12_000, 'hiking'), ride(new Date(2026, 11, 31, 23, 59), 50_000)];
        expect(yearTotals(rides, 2025)).toEqual({ lengthM: 40_000, rides: 1, hikes: 0 });
        expect(yearTotals(rides, 2026)).toEqual({ lengthM: 92_000, rides: 2, hikes: 1 });
    });
});
