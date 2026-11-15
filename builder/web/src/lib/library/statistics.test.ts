import { describe, expect, it } from 'vitest';
import type { Bike, RideRecord, RideSummary } from './records';
import { byKind, byMonth, byYear, largest, yearTotals } from './statistics';
import type { Trip } from './trips';

/** The start is made from local time parts, so each case holds in each time zone. */
const ride = (key: string, start: Date, lengthM: number, bike: Bike = 'road', summary: Partial<RideSummary> = {}) =>
    ({ key, name: key, summary: { start: start.getTime() / 1000, lengthM, climbM: 0, movingS: 3600, highestM: null, bike, ...summary } }) as RideRecord;
const trip = (...rides: RideRecord[]): Trip => ({ id: rides[0].key, name: rides[0].name, rides, days: rides.map((_, i) => i + 1) });

describe('the totals', () => {
    const rides = [ride('eve', new Date(2025, 11, 31, 23, 30), 40_000), ride('new year', new Date(2026, 0, 1, 0, 30), 30_000, 'gravel'),
        ride('walk', new Date(2026, 5, 1), 12_000, 'hiking'), ride('last', new Date(2026, 11, 31, 23, 59), 50_000)];

    it('never add a ride by bike and a ride on foot, and count a ride in the year of its local start', () => {
        expect(yearTotals(rides, 2025)).toMatchObject({ bike: { lengthM: 40_000, count: 1 }, foot: { lengthM: 0, count: 0 } });
        expect(yearTotals(rides, 2026)).toMatchObject({ bike: { lengthM: 80_000, movingS: 7200, count: 2 }, foot: { lengthM: 12_000, count: 1 } });
    });

    it('give each year its place up to this year, the 12 months of a year, and each kind of bike', () => {
        const old = [ride('old', new Date(2023, 4, 2), 10_000), ...rides];
        expect(byYear(old, 2027).map(({ year, totals }) => [year, totals.count])).toEqual([[2023, 1], [2024, 0], [2025, 1], [2026, 3], [2027, 0]]);
        expect(byMonth(old, 2026).map(({ totals }) => totals.lengthM)).toEqual([30_000, 0, 0, 0, 0, 12_000, 0, 0, 0, 0, 0, 50_000]);
        expect([...byKind(rides)].map(([bike, totals]) => [bike, totals.lengthM])).toEqual([['road', 90_000], ['gravel', 30_000], ['hiking', 12_000]]);
        expect(byYear([rides[0]], 2025).map(({ year, kinds }) => [year, [...kinds.keys()]])).toEqual([[2025, ['road']]]);
    });
});

describe('largest', () => {
    it('adds the climb of the rides of one local day, and names the ride of that day with the most climb', () => {
        const rides = [ride('one big', new Date(2026, 4, 1, 9), 80_000, 'road', { climbM: 900, highestM: 1200 }),
            ride('morning', new Date(2026, 4, 2, 0, 10), 30_000, 'road', { climbM: 400, highestM: 2100 }),
            ride('evening', new Date(2026, 4, 2, 23, 50), 30_000, 'road', { climbM: 600 })];
        expect(largest(rides, [])).toEqual({ ride: rides[0], day: { climbM: 1000, ride: rides[2] }, trip: undefined, highest: rides[1] });
        expect(largest([ride('flat', new Date(2026, 4, 1), 5000)], [])).toMatchObject({ day: undefined, highest: undefined });
    });

    it('counts a trip with rides by bike and on foot in each statistic with its own rides', () => {
        const days = [ride('day 1', new Date(2026, 6, 1), 90_000), ride('day 2', new Date(2026, 6, 2), 14_000, 'hiking'), ride('day 3', new Date(2026, 6, 3), 70_000)];
        const mixed = trip(...days), short = trip(ride('other 1', new Date(2026, 7, 1), 60_000), ride('other 2', new Date(2026, 7, 2), 60_000));
        expect(largest([days[0], days[2], ...short.rides], [short, mixed]).trip).toEqual({ trip: mixed, lengthM: 160_000 });
        expect(largest([days[1]], [short, mixed]).trip).toEqual({ trip: mixed, lengthM: 14_000 });
    });

    it('counts a trip over the turn of a year for the year of its first ride', () => {
        const over = trip(ride('eve', new Date(2025, 11, 31, 10), 50_000), ride('new year', new Date(2026, 0, 1, 10), 50_000));
        expect(largest(over.rides, [over], 2025).trip).toEqual({ trip: over, lengthM: 100_000 });
        expect(largest(over.rides, [over], 2026)).toMatchObject({ ride: over.rides[1], trip: undefined });
    });
});
