import { describe, expect, it } from 'vitest';
import type { RideRecord } from '../../lib/library/records';
import type { Trip } from '../../lib/library/trips';
import { rideItem } from './filter';
import { rideRows } from './rows';

const ride = (key: string, start: number) => ({ key, name: `Ride ${key}`, summary: { start, lengthM: 1000 } }) as RideRecord;

describe('rideRows', () => {
    const [a, b, c, d, e, f] = ['a', 'b', 'c', 'd', 'e', 'f'].map((key, i) => ride(key, (i + 1) * 86_400));
    const trip: Trip = { id: 'trip', name: 'Alps', rides: [a, b], days: [1, 2] };
    const groups = new Map([[c.key, 'route'], [d.key, 'route'], [e.key, e.key], [f.key, 'route']]);
    const names = new Map([['route', 'Lake loop']]);

    it('shows a whole trip for one shown ride, a group of 2 shown rides with the route name, and other rides alone, newest first', () => {
        const rows = rideRows([b, c, d, e, f].map(rideItem), [trip], groups, names);
        expect(rows.map(row => [row.kind, row.id, row.name, row.rides.map(item => item.id)])).toEqual([
            ['group', 'route', 'Lake loop', ['f', 'd', 'c']],
            ['ride', 'e', 'Ride e', ['e']],
            ['trip', 'trip', 'Alps', ['a', 'b']],
        ]);
        expect(rows[2].days).toBe(2);
    });

    it('shows a group with one shown ride as that ride', () => {
        expect(rideRows([c, e].map(rideItem), [trip], groups, names).map(row => [row.kind, row.id])).toEqual([['ride', 'e'], ['ride', 'c']]);
    });
});
