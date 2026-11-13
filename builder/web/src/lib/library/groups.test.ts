import { describe, expect, it } from 'vitest';
import { groups, sameRoute } from './groups';
import { rideKeyAt } from './import';
import type { RideRecord, RouteRecord } from './records';
import { rideSummary } from './summary';

const METRES_PER_DEGREE = 6_371_008.8 * Math.PI / 180, COS = Math.cos(47 * Math.PI / 180);

/** A wiggly loop of about 30 km around 8° E, 47° N, as points in metres every 5 m, from `from` (0 to 1) of the way round. */
function loop(from = 0, backwards = false, shift = 0): [number, number][] {
    const n = 6000;
    return Array.from({ length: n + 1 }, (_, i) => {
        const angle = 2 * Math.PI * (from + (backwards ? -i : i) / n), radius = 4600 + 300 * Math.sin(7 * angle);
        return [radius * Math.cos(angle), radius * Math.sin(angle) + shift];
    });
}

/** GPS noise of up to 15 m that drifts over a few hundred metres, as a receiver's error does. */
const noisy = (points: [number, number][], seed: number): [number, number][] =>
    points.map(([x, y], i) => [x + 15 * Math.sin(i / (17 + seed) + seed), y + 15 * Math.cos(i / (23 + seed) + 2 * seed)]);

function ride(points: [number, number][], start: number): RideRecord {
    const summary = rideSummary(points.map(([x, y], i) => ({
        timeS: start + i, lon: Math.round((8 + x / METRES_PER_DEGREE / COS) * 1e6), lat: Math.round((47 + y / METRES_PER_DEGREE) * 1e6),
        elevationM: null, segmentStart: i === 0,
    })), 'gravel');
    return { key: rideKeyAt(start), name: `Ride ${start}`, source: 'file', format: 'gpx', summary, maxHr: null, ftp: null };
}

const route = (id: string, updatedAt: number, from: RideRecord): RouteRecord =>
    ({ id, revision: 0, name: id, updatedAt, summary: { ...from.summary, days: 1 }, trip: {} });

describe('sameRoute', () => {
    const a = ride(noisy(loop(), 1), 1000).summary;

    it('joins two noisy rides of one road, from another start and the other way', () => {
        expect(a.line.length / 2).toBeLessThanOrEqual(1000);
        expect(sameRoute(a, ride(noisy(loop(), 2), 2000).summary)).toBe(true);
        expect(sameRoute(a, ride(noisy(loop(0.4, true), 3), 3000).summary)).toBe(true);
    });

    it('keeps apart a ride 12 % longer, a road 150 m away and a ride that is 15 % elsewhere', () => {
        const longer = loop();
        expect(sameRoute(a, ride([...longer, ...loop().slice(0, 720)], 2000).summary)).toBe(false);
        expect(sameRoute(a, ride(loop(0, false, 150), 3000).summary)).toBe(false);
        // 15 % of the loop moves 400 m out; the length changes by less than 10 %.
        const elsewhere = loop().map(([x, y], i): [number, number] => i < 900 ? [x * (1 + 400 / 4600), y * (1 + 400 / 4600)] : [x, y]);
        const other = ride(elsewhere, 4000).summary;
        expect(Math.abs(other.lengthM - a.lengthM) / a.lengthM).toBeLessThan(0.1);
        expect(sameRoute(a, other)).toBe(false);
    });
});

describe('groups', () => {
    const [first, second, third] = [1, 2, 3].map(seed => ride(noisy(loop(seed / 10), seed), seed * 86_400));
    const other = ride(loop(0, false, 5000), 4 * 86_400);

    it('puts the rides of one road in the newest route that they follow, else in the group of the first ride', async () => {
        const routes = [route('old', 1, first), route('new', 2, second), route('far', 3, other)];
        expect(await groups([third, second, first, other], routes)).toEqual(new Map([
            [first.key, 'new'], [second.key, 'new'], [third.key, 'new'], [other.key, 'far']]));
        expect(await groups([third, second, first, other], [])).toEqual(new Map([
            [first.key, first.key], [second.key, first.key], [third.key, first.key], [other.key, other.key]]));
    });

    it('leaves out a ride taken apart, and gives the group to the next ride when the first is deleted', async () => {
        expect(await groups([first, second, third], [], [second.key])).toEqual(new Map([[first.key, first.key], [third.key, first.key]]));
        expect(await groups([second, third], [])).toEqual(new Map([[second.key, second.key], [third.key, second.key]]));
    });
});
