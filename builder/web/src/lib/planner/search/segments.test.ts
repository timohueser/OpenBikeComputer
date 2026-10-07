import { describe, expect, it } from 'vitest';
import { cumulative, type Coordinate } from '../geo';
import type { Edges } from '../routing';
import { routeSegments } from './segments';

describe('route segments for search', () => {
    // Five 500 m edges.
    const coordinates = Array.from({ length: 6 }, (_, i): Coordinate => [8, 48 + i * .5 / 111.195]);
    const km = cumulative(coordinates).map(value => Math.round(value * 1000) / 1000);
    it('merges adjacent edges of one kind, and a manual leg gives no edge facts', () => {
        // The last edge is a manual leg: it has no pushing value. The edge before it is routed, so its missing surface is unknown.
        const edges: Edges = {
            surfaces: ['Paved', 'Gravel', 'Dirt', null, null],
            pushing: [false, false, true, true, null],
            closures: [null, [{ kind: 'private', condition: '' }], null, null, null],
        };
        expect(routeSegments({ coordinates, elevation: [100, 130, 160, 220, 250, 250], edges })).toEqual([
            { kind: 'climb', from: 0, to: km[4], ascent: 150, gradient: 7.5 },
            { kind: 'steep', from: km[2], to: km[3], ascent: 60, gradient: 12 },
            { kind: 'unpaved', from: km[1], to: km[3] },
            { kind: 'unknown_surface', from: km[3], to: km[4] },
            { kind: 'pushing', from: km[2], to: km[4] },
            { kind: 'closure', from: km[1], to: km[2] },
        ]);
    });
    it('finds climbs and descents with the device rule, and steep runs only uphill', () => {
        expect(routeSegments({ coordinates, elevation: [250, 250, 220, 160, 130, 100], edges: {} })).toEqual([
            { kind: 'descent', from: 0, to: km[5], ascent: 150, gradient: 6 },
        ]);
        expect(routeSegments({ coordinates, elevation: [100, 104, 100, 104, 100, 104], edges: {} })).toEqual([]);
    });
});
