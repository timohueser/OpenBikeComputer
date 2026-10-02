import { afterEach, describe, expect, it, vi } from 'vitest';
import { buildQueryRoute } from './route-client';
import type { AnswerRoute } from '../route-answer';

const points = [{ coordinate: [8, 48] as [number, number], label: 'A' }, { coordinate: [8.1, 48] as [number, number], label: 'B' }];
const route: AnswerRoute = {
    id: 'route', reason: 'primary', package: 'test', profile: 'gravel/less-climbing', coordinates_udeg: [8_001_000, 48_000_000, 99_000, 0],
    elevation_dm: [0, 0], elapsed_s: [0, 10], surfaces: [['Paved', 1]], pushing: [[false, 1]], legs: [{ from_index: 0, to_index: 1 }], snap_truncated: true,
    totals: { distance_m: 7000, ascent_m: 0, seconds: 10, surface_m: [0, 7000, 0, 0, 0, 0], unknown_elevation_m: 0, pushing_m: 0 },
};
const answer = (routes: AnswerRoute[]) => vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: true, json: async () => ({ routes }) }));
afterEach(() => vi.unstubAllGlobals());

describe('query route', () => {
    it('keeps snapped connectors explicit and rejects incomplete legs', async () => {
        answer([route]);
        const notice = vi.fn();
        expect(await buildQueryRoute(points, 'gravel', 'least_climbing', notice)).toEqual([[[8, 48], [8.001, 48], [8.1, 48]]]);
        expect(notice).toHaveBeenCalledWith('The routing engine reached its snapping search limit. Connections from the selected places to snapped roads are straight and unverified.');
        answer([{ ...route, legs: [{ from_index: 0, to_index: 5 }] }]);
        await expect(buildQueryRoute(points, 'gravel', 'balanced')).rejects.toThrow('incomplete legs');
    });
});
