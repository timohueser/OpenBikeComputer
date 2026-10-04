import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { decodeRoutes } from './route-answer';
import type { RouteLeg, RouteTotals } from './routing';

const vector = JSON.parse(readFileSync(new URL('../../../../../specs/vectors/route-answer.json', import.meta.url), 'utf8'));

describe('route answer', () => {
    it('decodes the shared vector to its source route within the specified precision', () => {
        const source = vector.route;
        const [route] = decodeRoutes(vector.answer);
        expect(route).toMatchObject({ id: source.id, reason: source.reason, package: source.package, profile: source.profile, snap_truncated: source.snap_truncated });
        expect(route.geometry).toEqual(source.geometry);
        expect(route.elevation.map(h => h === null)).toEqual(source.elevation.map((h: number | null) => h === null));
        route.elevation.forEach((h, i) => h !== null && expect(Math.abs(h - source.elevation[i])).toBeLessThanOrEqual(0.05));
        route.elapsed.forEach((t, i) => expect(Math.abs(t - source.elapsed[i])).toBeLessThanOrEqual(0.5));
        expect(route.edges).toEqual(source.edges);
        expect(route.legs.map(leg => [leg.from_index, leg.to_index])).toEqual(source.legs.map((leg: RouteLeg) => [leg.from_index, leg.to_index]));
        const expectTotals = (totals: RouteTotals, { seconds, descent_m: _descent, ...exact }: RouteTotals & Record<string, number>) => {
            expect(totals).toMatchObject(exact);
            expect(Math.abs(totals.seconds - seconds)).toBeLessThanOrEqual(0.5);
        };
        expectTotals(route.totals, source.totals);
        route.legs.forEach((leg, k) => expectTotals(leg.totals, source.legs[k].totals));
        expect(() => decodeRoutes({ routes: [{ ...vector.answer.routes[0], elapsed_s: [0] }] })).toThrow('invalid response');
        // A missing channel stays missing, which reads as null on every edge; an unknown channel is kept.
        const { surfaces: _missing, ...edges } = vector.answer.routes[0].edges;
        const [partial] = decodeRoutes({ routes: [{ ...vector.answer.routes[0], edges: { ...edges, future: [[7, 5]] } }] });
        expect(partial.edges).toEqual({ ...source.edges, surfaces: undefined, future: [7, 7, 7, 7, 7] });
        expect(() => decodeRoutes({ routes: [{ ...vector.answer.routes[0], edges: { future: [[7, 4]] } }] })).toThrow('invalid response');
    });
});
