import { describe, expect, it } from 'vitest';
import { rideKeyAt } from './import';
import type { RideRecord, RideTrip } from './records';
import { rideSummary } from './summary';
import { trips } from './trips';

const KM_PER_DEGREE = 6_371.0088 * Math.PI / 180, COS = Math.cos(47 * Math.PI / 180);

/** A straight ride of 3 hours from `from` to `to`, in km east and north of 8° E, 47° N, that starts at `start`, local time. */
function ride(start: Date, from: [number, number], to: [number, number], trip?: RideTrip): RideRecord {
    const s = start.getTime() / 1000;
    const summary = rideSummary([0, 0.5, 1].map((t, i) => ({
        timeS: s + i * 5400, lon: Math.round((8 + (from[0] + t * (to[0] - from[0])) / KM_PER_DEGREE / COS) * 1e6),
        lat: Math.round((47 + (from[1] + t * (to[1] - from[1])) / KM_PER_DEGREE) * 1e6), elevationM: null, segmentStart: i === 0,
    })), 'touring');
    return { key: rideKeyAt(s), name: `${start.getDate()}.`, source: trip ? 'device' : 'file', format: 'gpx', summary, maxHr: null, ftp: null, ...trip && { trip } };
}

const at = (day: number, hour = 9) => new Date(2026, 6, day, hour);
const shape = (list: RideRecord[]) => trips(list).map(trip => ({ id: trip.id, name: trip.name, rides: trip.rides.length, days: trip.days }));

describe('trips', () => {
    it('keeps a trip of the device across a train day, with its name, and the first ride names one with no name', () => {
        const alps = { key: '9223372036854775813', name: 'Alps' };
        const list = [ride(at(1), [0, 0], [80, 0], { ...alps, day: 1 }), ride(at(3), [300, 0], [380, 0], { ...alps, day: 3 }),
            ride(at(10), [0, 0], [80, 0], { key: '5', name: '', day: 1 }), ride(at(11), [80, 0], [160, 0], { key: '5', name: '', day: 2 })];
        expect(shape(list)).toEqual([
            { id: alps.key, name: 'Alps', rides: 2, days: [1, 3] },
            { id: '5', name: '10.', rides: 2, days: [1, 2] },
        ]);
    });

    it('joins the rides that go on from the end of the ride before, across a rest day, and not across a train day', () => {
        const list = [ride(at(1), [0, 0], [90, 0]), ride(at(1, 15), [91, 0], [120, 0]), ride(at(2), [120, 2], [200, 0]),
            ride(at(4), [203, 0], [280, 0]), ride(at(5), [600, 0], [680, 0])];
        expect(shape(list)).toEqual([{ id: list[0].key, name: '1.', rides: 4, days: [1, 1, 2, 4] }]);
    });

    it('makes no trip of a commute week or of loops from one base', () => {
        const commute = [1, 2, 3, 4, 5].flatMap(day => [ride(at(day, 7), [0, 0], [12, 0]), ride(at(day, 17), [12, 0], [0, 0])]);
        expect(trips(commute)).toEqual([]);
        const base = [6, 7, 8].map(day => ride(at(day), [500, 0], [500.5, 0]));
        expect(trips(base)).toEqual([]);
    });
});
