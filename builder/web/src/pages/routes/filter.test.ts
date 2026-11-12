// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { readSettings, updateSettings } from '../../lib/settings/settings.svelte';
import { distanceToBox, results, routeItem, type Item } from './filter';

const stored = new Map<string, string>();
beforeEach(() => {
    stored.clear();
    vi.stubGlobal('localStorage', { getItem: (key: string) => stored.get(key) ?? null, setItem: (key: string, value: string) => stored.set(key, value) });
    updateSettings(readSettings(null));
});

// A line along the meridian 8° E, from 47° N up 0.1°, about 11 km.
const item = (id: string, fields: Partial<NonNullable<Item['summary']>> = {}, name = id, updatedAt = 0): Item => ({
    id, name, updatedAt,
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
        const broken: Item = { id: 'broken', name: 'Old plan', summary: null, updatedAt: 0 };
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
});

describe('distanceToBox', () => {
    it('is zero for a segment that crosses a box with no point in it, and measures to a point', () => {
        expect(distanceToBox([7.9, 47.05, 8.1, 47.05], [8, 47, 8.01, 47.1])).toBe(0);
        // 0.01° of latitude is 1.11 km.
        expect(distanceToBox([8, 47, 8, 47.1], [8, 47.11, 8, 47.11])).toBeCloseTo(1112, -1);
    });
});

describe('routeItem', () => {
    const route = { id: 'r', revision: 1, name: '', updatedAt: 5, summary: item('r').summary as never, trip: { points: [] } };
    it('names a plan with no name by its title, and a route that is not a plan', () => {
        expect(routeItem(route, { isPlan: () => true, planTitle: () => 'Loop from Bern' }).name).toBe('Loop from Bern');
        expect(routeItem(route, { isPlan: () => false, planTitle: () => '' })).toEqual({ id: 'r', name: 'Route that cannot be read', summary: null, updatedAt: 5 });
    });
});
