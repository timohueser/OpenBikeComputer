import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Coordinate } from './geo';
import { townName } from './point-names';

afterEach(() => vi.unstubAllGlobals());

describe('point names', () => {
    it('names a point after the nearest city, town or village within 3 km', async () => {
        const place = (name: string, kind: string, distance: number) => ({ name, kind, distance });
        const fetch = vi.fn(async (_url: string, _init: RequestInit) => ({ ok: true, json: async () => ({ type: 'places',
            results: [place('Oberneuhäuser', 'hamlet', .4), place('Kirchzarten', 'village', .8), place('Freiburg', 'city', 2.9)] }) }));
        vi.stubGlobal('fetch', fetch);
        const at: Coordinate = [8.05, 48.02];
        expect(await townName(at)).toBe('Kirchzarten');
        const { view, request } = JSON.parse(fetch.mock.calls[0][1].body as string);
        expect([(view[0] + view[2]) / 2, (view[1] + view[3]) / 2].map(n => n.toFixed(6))).toEqual(at.map(n => n.toFixed(6)));
        expect(request).toEqual({ type: 'places', what: ['town'], radius: { value: 3, unit: 'km' } });
    });
});
