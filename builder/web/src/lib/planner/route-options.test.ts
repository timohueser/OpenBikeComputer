import { describe, expect, it } from 'vitest';
import type { Coordinate } from './geo';
import { difference, divergence, optionName, optionOrder } from './route-options';
import type { EngineRoute } from './routing';

function route(id: string, reason: string, profile: string, km: number, climb: number, unpavedKm: number, unknownElevation = 0, via?: Coordinate): EngineRoute {
    // The service's ascent runs low, so the list measures the climb on the heights.
    return { id, reason, profile, package: 'test', geometry: [[8, 48], [8.01, 48]], elevation: [100, 100 + climb], elapsed: [], edges: {}, legs: [], snap_truncated: false, via,
        totals: { distance_m: km * 1000, ascent_m: climb - 5, seconds: 0, surface_m: [0, km * 1000 - unpavedKm * 1000, 0, unpavedKm * 1000, 0, 0], unknown_elevation_m: unknownElevation, pushing_m: 0 } };
}

describe('route options', () => {
    const balanced = route('a', 'primary', 'touring', 30.7, 906, 2.4);
    const shorter = route('b', 'shorter', 'touring/shorter', 25.2, 669, 2.4);
    const corridor = route('c', 'corridor', 'touring', 30.1, 886, 2.5, 0, [8, 48]);

    it('compares every option with the current route, which leads the list', () => {
        expect(optionOrder([balanced, shorter, corridor], 'b')).toEqual([shorter, balanced, corridor]);
        expect(difference(shorter, balanced)).toBe('5.5 km shorter · 237 m less climbing');
        expect(difference(balanced, shorter)).toBe('5.5 km longer · 237 m more climbing');
        expect(difference(corridor, balanced)).toBe('0.6 km shorter · 20 m less climbing · 0.1 km more unpaved');
        expect(difference(route('e', 'smoother', 'touring', 30.72, 906, 2.4), balanced)).toBe('Same distance · same climbing');
        expect(difference(shorter, route('d', 'primary', 'touring', 30, 900, 2.4, 120))).toBe('4.8 km shorter');
    });

    it('names options by their reason, and a corridor by its settlement or as another way', () => {
        expect([balanced, shorter, route('s', 'smoother', 'touring', 1, 0, 0), route('l', 'less_climbing', 'touring/less-climbing', 1, 0, 0)]
            .map(r => optionName(r, {}))).toEqual(['Balanced', 'Shorter', 'Smoother', 'Less climbing']);
        expect([optionName(corridor, { c: 'Kirchzarten' }), optionName(corridor, {})]).toEqual(['Via Kirchzarten', 'Other way']);
    });

    it('marks a line in the middle of its longest stretch away from the other line', () => {
        const shared: Coordinate[] = [[8, 48], [8.01, 48], [8.02, 48]];
        const valley: Coordinate[] = [...shared, [8.03, 48.01], [8.04, 48.01], [8.05, 48]];
        const ridge: Coordinate[] = [...shared, [8.035, 48], [8.05, 48]];
        expect(divergence(valley, ridge)).toEqual([8.035, 48.01]);
        expect(divergence(shared, valley)).toBeUndefined();
    });
});
