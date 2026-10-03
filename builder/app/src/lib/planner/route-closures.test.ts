import { describe, expect, it } from 'vitest';
import { closureNote, closureStretches } from './route-closures';
import { selectRoute } from './routing';
import { decodeRoutes } from './route-answer';
import { initialTrip } from './editor';

describe('seasonal closures', () => {
    it('joins adjacent closures into one stretch and names their conditions in one quiet line', () => {
        // Six points along the equator, 0.01° (1.11 km) apart; three adjacent edges cross two closures.
        const [route] = decodeRoutes({ routes: [{
            id: 'r', reason: 'primary', package: 'p', profile: 'touring', snap_truncated: false, legs: [],
            coordinates_udeg: [0, 0, ...Array(5).fill([10_000, 0]).flat()], elevation_dm: Array(6).fill(null), elapsed_s: Array(6).fill(0),
            surfaces: [['Paved', 5]], pushing: [[false, 5]], closures: [[null, 1], ['dec-mar', 1], ['Nov-May', 2], [null, 1]],
            totals: { distance_m: 0, ascent_m: 0, seconds: 0, surface_m: [0, 0, 0, 0, 0, 0], unknown_elevation_m: 0, pushing_m: 0 },
        }] });
        const line = selectRoute(initialTrip(), route, [route]);
        const [stretch, ...others] = closureStretches(line);
        expect(others).toEqual([]);
        expect(stretch.coordinates).toEqual([[0.01, 0], [0.02, 0], [0.03, 0], [0.04, 0]]);
        expect(closureNote([stretch])).toBe('3.3 km may be closed seasonally (Dec–Mar, Nov–May)');
        expect(closureNote([{ ...stretch, km: 0.4, conditions: ['mar 1 - jul 31'] }])).toBe('400 m may be closed seasonally (Mar 1–Jul 31)');
        expect(closureStretches({ ...line, closures: line.closures.map(() => null) })).toEqual([]);
    });
});
