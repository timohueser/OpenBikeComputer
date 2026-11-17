// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { readSettings, updateSettings } from '../../lib/settings/settings.svelte';
import type { RouteSummary } from '../../lib/library/records';
import type { CatalogRecord } from '../../lib/planner/signed-routes';
import { distanceToBox, readSearch, results, rideItem, routeItem, scale, signedItem, type Item } from './filter';

vi.mock('../../lib/planner/search/types', () => ({
    searchPlaces: async () => ({ type: 'places', request: {}, results: [
        { name: 'Zürichberg', kind: 'hill', lon: 8.57, lat: 47.38 },
        { name: 'Zürich', kind: 'city', lon: 8.54, lat: 47.37, west: 8.45, south: 47.32, east: 8.63, north: 47.43 },
    ] }),
}));

const stored = new Map<string, string>();
beforeEach(() => {
    stored.clear();
    vi.stubGlobal('localStorage', { getItem: (key: string) => stored.get(key) ?? null, setItem: (key: string, value: string) => stored.set(key, value) });
    updateSettings(readSettings(null));
});

// A line along the meridian 8° E, from 47° N up 0.1°, about 11 km.
const item = (id: string, fields: Partial<RouteSummary> = {}, name = id, at = 0): Item => ({
    kind: 'route', id, name, at,
    summary: { line: [8, 47, 8, 47.1], heights: [400, 500], bounds: [8, 47, 8, 47.1], lengthM: 11_000, climbM: 100, highestM: 500, seconds: 3600, bike: 'gravel', days: 1, ...fields },
});
const ids = (list: Item[]) => list.map(entry => entry.id);

describe('results', () => {
    it('keeps the items of each chip, in kilometres and in miles', () => {
        const items = [item('short', { lengthM: 90_000, climbM: 400 }), item('long', { lengthM: 150_000, climbM: 600, bike: 'road', days: 3 })];
        expect(ids(results(items, { length: [0, 100] }, ''))).toEqual(['short']);
        expect(ids(results(items, { climb: [500, 1500] }, ''))).toEqual(['long']);
        expect(ids(results(items, { bike: 'road' }, ''))).toEqual(['long']);
        expect(ids(results(items, { days: [2, 5] }, ''))).toEqual(['long']);
        updateSettings({ units: 'mi' });
        // 150 km is 93 mi; 600 m is 1,969 ft.
        expect(ids(results(items, { length: [0, 100] }, ''))).toEqual(['short', 'long']);
        expect(ids(results(items, { climb: [1500, 5000] }, ''))).toEqual(['long']);
    });

    it('matches the name with no case and no accents', () => {
        const items = [item('a', {}, 'Brünig Pass'), item('b', {}, 'Lake loop')];
        expect(ids(results(items, {}, 'brunig'))).toEqual(['a']);
        expect(ids(results(items, {}, '  LAKE '))).toEqual(['b']);
    });

    it('keeps a route with no summary for a text, and leaves it out with a chip or the view', () => {
        const broken: Item = { kind: 'route', id: 'broken', name: 'Old plan', summary: null, at: 0 };
        expect(ids(results([broken], {}, 'old'))).toEqual(['broken']);
        expect(results([broken], { days: [1, 2] }, '')).toEqual([]);
        expect(results([broken], {}, '', [0, 0, 10, 50])).toEqual([]);
    });

    it('keeps the lines near a place by its kind, the nearest first and the last changed for a tie', () => {
        // 0.05° of longitude at 47° N is 3.8 km.
        const items = [item('on', {}, 'on', 1), item('also on', {}, 'also on', 2), item('east', { line: [8.05, 47, 8.05, 47.1] }, 'east', 3)];
        const village = { name: 'V', kind: 'village', box: [8, 47.05, 8, 47.05] as [number, number, number, number] };
        expect(ids(results(items, { place: village }, ''))).toEqual(['also on', 'on']);
        expect(ids(results(items, { place: { ...village, kind: 'town' } }, ''))).toEqual(['also on', 'on', 'east']);
        expect(ids(results(items, {}, ''))).toEqual(['east', 'also on', 'on']);
    });

    it('with a view, keeps the items with a point in it', () => {
        const items = [item('in'), item('out', { line: [9, 47, 9, 47.1] })];
        expect(ids(results(items, {}, '', [7.9, 46.9, 8.1, 47.01]))).toEqual(['in']);
    });

    it('keeps the rides of the year chip by their local start', () => {
        const ride = (key: string, start: Date) => rideItem({ key, name: key, source: 'file', format: 'gpx', maxHr: null, ftp: null,
            summary: { ...item(key).summary!, start: start.getTime() / 1000, movingS: 3000 } });
        const items = [ride('eve', new Date(2025, 11, 31, 23, 30)), ride('new', new Date(2026, 0, 1, 0, 30))];
        expect(ids(results(items, { year: 2026 }, ''))).toEqual(['new']);
        expect(ids(results(items, {}, ''))).toEqual(['new', 'eve']);
    });
});

// A route of 450 m whose simplified line runs 300 m north along 8° E, with one vertex after its first 100 m.
const record = (fields: Partial<CatalogRecord> = {}): CatalogRecord => ({
    id: 7, kind: 'hiking', ref: 'R7', rank: 2, loop: false, length_m: 450, ascent_m: 120, descent_m: 0, cells: ['9-267-180'],
    line_udeg: [8_000_000, 47_000_000, 0, 900, 0, 1800], via: [1], profile_m: [500, 510, 530, 560, 600, 620], ...fields,
});

describe('signedItem', () => {
    it('reads the height of each point from the profile at its share of the line, and places the entries on the line', () => {
        const item = signedItem(record(), 'hiking');
        expect(item).toMatchObject({ kind: 'signed', id: '7', name: 'R7', at: 0 });
        // The vertex at a third of the line is at 150 m of the route, between the entries of 100 and 200 m.
        expect(item.summary).toEqual({ line: [8, 47, 8, 47.0009, 8, 47.0027], heights: [500, 520, 620], bounds: [8, 47, 8, 47.0027],
            lengthM: 450, climbM: 120, highestM: 620, seconds: 0, bike: 'hiking', days: 1 });
        // The last entry is at the end, 50 m after the one before it.
        expect(item.profile).toEqual({ line: [8, 47, 8, 47.0006, 8, 47.0012, 8, 47.0018, 8, 47.0024, 8, 47.0027], heights: [500, 510, 530, 560, 600, 620] });
        expect(signedItem(record({ profile_m: [500, null, 530, 560, 600, 620] }), 'hiking').summary.heights).toEqual([500, null, 620]);
    });

    it('has no heights and no profile without `profile_m`', () => {
        const item = signedItem(record({ profile_m: undefined, name: 'Lake path' }), 'hiking');
        expect(item.name).toBe('Lake path');
        expect(item.summary).toMatchObject({ heights: [null, null, null], highestM: null });
        expect(item.profile).toBeUndefined();
    });
});

describe('results of signed routes', () => {
    const signed = (id: number, fields: Partial<CatalogRecord>) => signedItem(record({ id, ...fields }), 'hiking');

    it('keeps the grade and the shape that the chips accept, and an ungraded route for each grade', () => {
        const items = [signed(1, { loop: true, hardest: 2 }), signed(2, { loop: false }), item('planned')];
        expect(ids(results(items, { grade: 1 }, ''))).toEqual(['2', 'planned']);
        expect(ids(results(items, { grade: 2, shape: 'loop' }, ''))).toEqual(['1', 'planned']);
        expect(ids(results(items, { shape: 'one-way' }, ''))).toEqual(['2', 'planned']);
    });

    it('in the order "shortest", sorts by the length and then by the number of the relation id', () => {
        const items = [signed(20, { length_m: 3000 }), signed(100, { length_m: 2000 }), signed(3, { length_m: 2000 })];
        expect(ids(results(items, {}, '', undefined, 'shortest'))).toEqual(['3', '100', '20']);
    });
});

describe('scale', () => {
    it('reaches above the value at 90 %, so a few very long values do not stretch it', () => {
        const values = [...Array.from({ length: 30 }, (_, i) => 20 + i * 70 / 29), 600, 1200, 2400];
        expect(scale(values)).toEqual({ step: 10, top: 160 });
    });

    it('reaches above each finite end of the range', () => {
        expect(scale([10, 20], [0, 300])).toEqual({ step: 20, top: 320 });
        expect(scale([10, 20], [50, Infinity])).toEqual({ step: 5, top: 80 });
    });

    it('has the step 1 with no value and no range', () => {
        expect(scale([])).toEqual({ step: 1, top: 16 });
    });
});

describe('distanceToBox', () => {
    it('is zero for a segment that crosses a box with no point in it, and measures to a point', () => {
        expect(distanceToBox([7.9, 47.05, 8.1, 47.05], [8, 47, 8.01, 47.1])).toBe(0);
        // 0.01° of latitude is 1.11 km.
        expect(distanceToBox([8, 47, 8, 47.1], [8, 47.11, 8, 47.11])).toBeCloseTo(1112, -1);
    });
});

describe('routeItem', () => {
    const route = { id: 'r', revision: 1, name: '', updatedAt: 5, summary: item('r').summary as RouteSummary, trip: { points: [] } };
    it('names a plan with no name by its title, and a route that is not a plan', () => {
        expect(routeItem(route, { isPlan: () => true, planTitle: () => 'Loop from Bern' }).name).toBe('Loop from Bern');
        expect(routeItem(route, { isPlan: () => false, planTitle: () => '' })).toEqual({ kind: 'route', id: 'r', name: 'Route that cannot be read', summary: null, at: 5 });
    });

    it('without the planner functions, names a route by the start and the finish of its trip', () => {
        expect(routeItem({ ...route, trip: { points: [{ kind: 'finish', label: 'Chur' }, { kind: 'start', label: 'Bern' }] } }).name).toBe('Bern → Chur');
        expect(routeItem(route).name).toBe('Route');
    });
});

describe('readSearch', () => {
    it('turns a place that the text names as whole words into a chip, and keeps the rest of the text', async () => {
        const signal = new AbortController().signal, view: [number, number, number, number] = [5, 45, 11, 48];
        expect(await readSearch('gravel zurich  loop', view, signal))
            .toEqual({ chips: { place: { name: 'Zürich', kind: 'city', box: [8.45, 47.32, 8.63, 47.43] } }, rest: 'gravel loop' });
        expect(await readSearch(' zurichsee loop ', view, signal)).toEqual({ chips: {}, rest: 'zurichsee loop' });
    });
});
