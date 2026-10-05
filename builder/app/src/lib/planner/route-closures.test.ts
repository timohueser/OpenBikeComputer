import { describe, expect, it } from 'vitest';
import { closureNote, closureStretches } from './route-closures';
import type { RouteClosure } from './routing';
import { decodeRoutes } from './route-answer';

const closure = (kind: RouteClosure['kind'], condition: string): RouteClosure => ({ kind, condition });

describe('possible closures', () => {
    it('joins adjacent closures into one stretch and names them in one quiet line', () => {
        // Six points along the equator, 0.01° (1.11 km) apart; three adjacent edges cross two closures.
        const permit = closure('permit', 'permit');
        const [route] = decodeRoutes({ routes: [{
            id: 'r', reason: 'primary', package: 'p', profile: 'touring', snap_truncated: false, legs: [],
            coordinates_udeg: [0, 0, ...Array(5).fill([10_000, 0]).flat()], elevation_dm: Array(6).fill(null), elapsed_s: Array(6).fill(0),
            edges: { closures: [[null, 1], [[permit, closure('seasonal', 'Oct 14th - May 31st')], 1], [[permit, closure('seasonal', 'oct 14-may 31')], 2], [null, 1]] },
            totals: { distance_m: 0, ascent_m: 0, seconds: 0, surface_m: [0, 0, 0, 0, 0, 0], unknown_elevation_m: 0, pushing_m: 0 },
        }] });
        const line = { coordinates: route.geometry, edges: route.edges };
        const [stretch, ...others] = closureStretches(line);
        expect(others).toEqual([]);
        expect(stretch.coordinates).toEqual([[0.01, 0], [0.02, 0], [0.03, 0], [0.04, 0]]);
        expect(closureNote([stretch])).toBe('Permit needed · May be closed seasonally (Oct 14–May 31) · 3.3 km');
        expect(closureStretches({ ...line, edges: {} })).toEqual([]);
    });

    it('words each kind honestly', () => {
        const note = (...closures: RouteClosure[]) => closureNote([{ closures, coordinates: [], km: 0.4 }]);
        expect(note(closure('seasonal', 'dec-mar'))).toBe('May be closed seasonally (Dec–Mar) · 400 m');
        expect(note(closure('conditional', 'wet'), closure('conditional', 'Mo-Fr 07:00-19:00')))
            .toBe('May be closed (wet, Mo–Fr 07:00–19:00) · 400 m');
        expect(note(closure('limited', 'destination'), closure('limited', 'customers;psv;motor_vehicle'))).toBe('Access for destination or customers or motor vehicles only · 400 m');
        expect(note(closure('unclear', 'unknown'))).toBe('Access unclear (unknown) · 400 m');
        expect(note(closure('private', 'private'), closure('farm', 'agricultural;forestry'), closure('sidepath', 'use_sidepath'), closure('discouraged', 'discouraged')))
            .toBe('Private road · Farm or forest traffic only · Use the side path · Access discouraged · 400 m');
    });
});
