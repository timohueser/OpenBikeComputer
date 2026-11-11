import { describe, expect, it } from 'vitest';
import { closureLabel, closureStretches, type ClosureStretch } from './route-closures';
import type { RouteClosure } from './routing';
import { decodeRoutes } from './route-answer';

const closure = <K extends RouteClosure['kind']>(kind: K, condition: string) => ({ kind, condition });

describe('possible closures', () => {
    it('joins adjacent closures into one stretch, skips side paths and names the stretch', () => {
        // Seven points along the equator, 0.01° (1.11 km) apart; three adjacent edges cross two closures, and a later edge only has a side path.
        const permit = closure('permit', 'permit');
        const sidepath = closure('sidepath', 'use_sidepath');
        const [route] = decodeRoutes({ routes: [{
            package: 'p', profile: 'touring', snap_truncated: false, legs: [],
            coordinates_udeg: [0, 0, ...Array(6).fill([10_000, 0]).flat()], elevation_dm: Array(7).fill(null), elapsed_s: Array(7).fill(0),
            edges: { closures: [[null, 1], [[permit, closure('seasonal', 'Oct 14th - May 31st')], 1], [[permit, sidepath, closure('seasonal', 'oct 14-may 31')], 2], [null, 1], [[sidepath], 1]] },
            totals: { distance_m: 0, ascent_m: 0, seconds: 0, surface_m: [0, 0, 0, 0, 0, 0], unknown_elevation_m: 0, pushing_m: 0 },
        }] });
        const line = { coordinates: route.geometry, edges: route.edges };
        const [stretch, ...others] = closureStretches(line);
        expect(others).toEqual([]);
        expect(stretch.coordinates).toEqual([[0.01, 0], [0.02, 0], [0.03, 0], [0.04, 0]]);
        expect(stretch.at).toBeCloseTo(1.11, 2);
        expect(closureLabel(stretch)).toBe('Permit needed · May be closed Oct 14–May 31');
        expect(closureStretches({ ...line, edges: {} })).toEqual([]);
    });

    it('words each kind honestly', () => {
        const label = (...closures: ClosureStretch['closures']) => closureLabel({ closures, coordinates: [], km: 0.4, at: 0 });
        expect(label(closure('seasonal', 'dec-mar'))).toBe('May be closed Dec–Mar');
        expect(label(closure('seasonal', ''))).toBe('May be closed seasonally');
        expect(label(closure('seasonal', 'Winter'))).toBe('May be closed in winter');
        expect(label(closure('seasonal', 'nov-may'), closure('conditional', 'wet'))).toBe('May be closed Nov–May · May be closed (wet)');
        expect(label(closure('conditional', 'wet'), closure('conditional', 'Mo-Fr 07:00-19:00'))).toBe('May be closed (Mo–Fr 07:00–19:00, wet)');
        expect(label(closure('conditional', 'snow'), closure('conditional', 'wet'))).toBe(label(closure('conditional', 'wet'), closure('conditional', 'snow')));
        expect(label(closure('limited', 'customers'))).toBe('Customers only');
        expect(label(closure('limited', 'residents'), closure('limited', 'customers;psv;motor_vehicle'))).toBe('Only for customers or motor vehicles or residents');
        expect(label(closure('unclear', 'unknown'))).toBe('Access unclear (unknown)');
        expect(label(closure('private', 'private'), closure('farm', 'agricultural;forestry'), closure('discouraged', 'discouraged')))
            .toBe('Private road · Farm or forest traffic only · Access discouraged');
    });
});
