import { describe, expect, it } from 'vitest';
import { alpineNote, alpineStretches } from './route-difficulty';
import { selectRoute } from './routing';
import { decodeRoutes } from './route-answer';
import { initialTrip } from './editor';

describe('alpine paths', () => {
    it('sums the T4 stretches into one quiet line and keeps T3 silent', () => {
        // Seven points along the equator, 0.01° (1.11 km) apart.
        const [route] = decodeRoutes({ routes: [{
            id: 'r', reason: 'primary', package: 'p', profile: 'hiking', snap_truncated: false, legs: [],
            coordinates_udeg: [0, 0, ...Array(6).fill([10_000, 0]).flat()], elevation_dm: Array(7).fill(null), elapsed_s: Array(7).fill(0),
            surfaces: [['Rough', 6]], pushing: [[false, 6]], closures: [[null, 6]], sac_scale: [[null, 1], [4, 2], [3, 1], [4, 1], [2, 1]],
            totals: { distance_m: 0, ascent_m: 0, seconds: 0, surface_m: [0, 0, 0, 0, 0, 0], unknown_elevation_m: 0, pushing_m: 0 },
        }] });
        const line = selectRoute(initialTrip(), route, [route]);
        const stretches = alpineStretches(line);
        expect(stretches.map(stretch => stretch.coordinates)).toEqual([[[0.01, 0], [0.02, 0], [0.03, 0]], [[0.04, 0], [0.05, 0]]]);
        expect(alpineNote(stretches)).toBe('3.3 km alpine path (T4): exposed, sure-footed hikers only');
        expect(alpineNote([{ coordinates: [], km: 0.4 }])).toBe('400 m alpine path (T4): exposed, sure-footed hikers only');
        expect(alpineStretches({ ...line, sacScale: line.sacScale.map(grade => grade && Math.min(grade, 3)) })).toEqual([]);
    });
});
