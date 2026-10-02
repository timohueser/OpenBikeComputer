import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { decodeRoutes } from './route-answer';

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
        expect([route.surfaces, route.pushing, route.legs]).toEqual([source.surfaces, source.pushing, source.legs]);
        const { seconds, descent_m: _descent, uncertain_access_m: _uncertain, ...exact } = source.totals;
        expect(route.totals).toMatchObject(exact);
        expect(Math.abs(route.totals.seconds - seconds)).toBeLessThanOrEqual(0.5);
        expect(() => decodeRoutes({ routes: [{ ...vector.answer.routes[0], elapsed_s: [0] }] })).toThrow('invalid response');
    });
});
