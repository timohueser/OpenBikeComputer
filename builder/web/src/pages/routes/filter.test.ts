// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { readSettings, updateSettings } from '../../lib/settings/settings.svelte';
import type { RouteSummary } from '../../lib/library/records';
import { longTrailId, type CatalogRecord, type TrailEntry } from '../../lib/planner/signed-routes';
import { distanceToBox, farOrder, farTrailId, nearTrails, readSearch, results, rideItem, routeItem, scale, signedItem, splitDay, trailItem, type Item } from './filter';

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
        // A null next entry is unknown too: no point takes the height of the entry before it.
        expect(signedItem(record({ profile_m: [500, 510, null, 560, 600, null] }), 'hiking').summary.heights).toEqual([500, null, null]);
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

describe('trailItem', () => {
    // Two pieces of a far line, each 0.01° north along 8° E, with a gap between them.
    const entry = (fields: Partial<TrailEntry> = {}): TrailEntry => ({
        id: 9, kind: 'hiking', name: 'Via Alpina', rank: 3, loop: false, length_m: 150_000, ascent_m: 9000, hardest: 2, stage_count: 20,
        far_udeg: [[8_000_000, 46_000_000, 0, 10_000], [8_000_000, 46_100_000, 0, 10_000]], ...fields,
    });

    it('joins the pieces of the far line in its summary, with the bounds of all pieces and no heights', () => {
        const item = trailItem(entry(), 'hiking');
        expect(item).toMatchObject({ kind: 'trail', id: '9', name: 'Via Alpina', at: 0, pieces: [[[8, 46], [8, 46.01]], [[8, 46.1], [8, 46.11]]] });
        expect(item.summary).toEqual({ line: [8, 46, 8, 46.01, 8, 46.1, 8, 46.11], heights: [null, null, null, null], bounds: [8, 46, 8, 46.11],
            lengthM: 150_000, climbM: 9000, highestM: null, seconds: 0, bike: 'hiking', days: 1 });
        // A section of a cut line has a negative id.
        expect(trailItem(entry({ id: -1234501 }), 'hiking').id).toBe('-1234501');
    });

    it('keeps the trails of the grade and the shape chips', () => {
        const items = [trailItem(entry({ id: 1, loop: true }), 'hiking'), trailItem(entry({ id: 2, hardest: 4 }), 'hiking')];
        expect(ids(results(items, { grade: 2 }, ''))).toEqual(['1']);
        expect(ids(results(items, { shape: 'one-way' }, ''))).toEqual(['2']);
    });

    it('in the far order, sorts by the network level, then the longest first, then the relation id', () => {
        const items = [entry({ id: 5, rank: 2, length_m: 400_000 }), entry({ id: 4, rank: 4, length_m: 50_000 }),
            entry({ id: 3, rank: 2, length_m: 90_000 }), entry({ id: 1, rank: 2, length_m: 90_000 })].map(trail => trailItem(trail, 'hiking'));
        expect(ids(items.sort(farOrder))).toEqual(['4', '5', '1', '3']);
    });
});

describe('the split of a near view into day routes and long trails', () => {
    // Trails 100 and 300 are in the far files; long routes 200 and 250 are no trails, and 250 is inside the complete trail 100.
    // 400 is a cut stage of trail 300; 500 is not loaded.
    const longs = [record({ id: 100, line_udeg: undefined, stages: [1, 6] }), record({ id: 200, line_udeg: undefined, stages: [3] }),
        record({ id: 250, line_udeg: undefined, stage_count: 1 }),
        record({ id: 300, line_udeg: undefined, stage_count: 5 }), record({ id: 400, line_udeg: undefined, trail: 300, trail_stage: 2 })];
    const recordOf = (id: number) => longs.find(long => long.id === id);
    const trail = (id: number) => ({ id, kind: 'hiking', rank: 3, loop: false, length_m: 90_000, ascent_m: 4000, far_udeg: [] }) as TrailEntry;
    const trails = new Map([100, 300, 600].map(id => [id, trail(id)]));
    const stage = record({ id: 1, trail: 100, trail_stage: 1, parent: 100, stage: 1 }), partial = record({ id: 2, trail: 300, trail_stage: 1 });
    const section = record({ id: -40001, trail: 400, section: 1 }), single = record({ id: 600 });
    const inner = record({ id: 6, trail: 250, trail_stage: 1, parent: 100, stage: 2 });
    const notTrail = record({ id: 3, trail: 200, trail_stage: 1, parent: 200, stage: 1 }), day = record({ id: 5 });
    const lost = record({ id: 4, trail: 500, trail_stage: 1, parent: 500, stage: 1 });
    const all = [stage, partial, section, single, inner, notTrail, lost, day];

    it('names the long trail of each record', () => {
        expect(all.map(one => longTrailId(one, recordOf))).toEqual([100, 300, 300, 600, 250, 200, undefined, 5]);
        // A stage of a long route that is no trail is in the complete trail of its `parent`.
        expect(all.map(one => farTrailId(one, trails, recordOf))).toEqual([100, 300, 300, 600, 100, undefined, undefined, undefined]);
    });

    it('moves a record whose long trail is a trail of the far files, and keeps the others as day routes', () => {
        const { day: days, long } = splitDay(all, trails, recordOf);
        expect(long).toEqual([stage, partial, section, single, inner]);
        expect(days).toEqual([notTrail, lost, day]);
        expect(splitDay(all, new Map(), recordOf)).toEqual({ day: all, long: [] });
    });

    it('lists each trail of the view once, and the trail of a moved record whose far line misses the view', () => {
        const near = nearTrails([trails.get(100)!, trail(700)], trails, [stage, partial, section], recordOf);
        expect(near.map(entry => entry.id)).toEqual([100, 700, 300]);
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
        expect(routeItem(route, { isPlan: () => true, planTitle: () => 'Loop from Bern' })).toMatchObject({ name: 'Loop from Bern', plan: true });
        expect(routeItem(route, { isPlan: () => false, planTitle: () => '' })).toEqual({ kind: 'route', id: 'r', name: 'Route that cannot be read', summary: null, at: 5 });
    });

    it('without the planner functions, names a route by the start and the finish of its trip', () => {
        expect(routeItem({ ...route, trip: { points: [{ kind: 'finish', label: 'Chur' }, { kind: 'start', label: 'Bern' }] } }).name).toBe('Bern → Chur');
        expect(routeItem(route)).toMatchObject({ name: 'Route' });
        expect(routeItem(route)).not.toHaveProperty('plan');
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
