import { describe, expect, it } from 'vitest';
import { kmPerDegree, type Coordinate } from '../../lib/planner/geo';
import { boxCells, MAX_CELLS, type Box } from '../../lib/planner/route-catalog';
import type { CatalogFacts, CatalogRecord, FactsPlace } from '../../lib/planner/signed-routes';
import { counts, dayStep, defaultQuery, driveCap, find, loadBox, openingQuery, queryParams, reachBox, readQuery, suggestions, type Loaded, type Query } from './search';

const TODAY = '2026-07-01';
const START: Coordinate = [8, 47];
/** The point `east` and `north` km from the start. */
const at = (east: number, north = 0): Coordinate => [START[0] + east / (kmPerDegree * Math.cos(47 * Math.PI / 180)), START[1] + north / kmPerDegree];
const udeg = ([lon, lat]: Coordinate): [number, number] => [Math.round(lon * 1e6), Math.round(lat * 1e6)];

/** A hiking route along `points`, with its line as `line_udeg` deltas. */
function route(id: number, points: Coordinate[], fields: Partial<CatalogRecord> = {}): CatalogRecord {
    const line = points.map(udeg).flatMap(([lon, lat], i, all) => i ? [lon - all[i - 1][0], lat - all[i - 1][1]] : [lon, lat]);
    return { id, kind: 'hiking', rank: 1, loop: true, length_m: 10_000, ascent_m: 600, descent_m: 600, moving_s: 14_400, hardest: 1, cells: [], line_udeg: line, ...fields };
}
const shares = { paved_pct: 10, big_road_pct: 0, water_pct: 0 };
const files = (records: CatalogRecord[], facts: Record<number, Partial<CatalogFacts>>, places: Record<string, FactsPlace> = {}): Loaded => ({
    records, factsOf: id => facts[id] && { ...shares, ...facts[id] }, place: id => places[id],
});
const query = (change: Partial<Query> = {}): Query => ({ ...defaultQuery(TODAY), ...change });
const rows = (found: ReturnType<typeof find>) => found.map(row => `${row.record.id} ${row.shape}`);

describe('reach', () => {
    it('caps the drive by latitude so that the reach meets at most 12 cells wherever the start lies', () => {
        expect([40, 46, 47, 50, 55, 60].map(driveCap)).toEqual([50, 45, 45, 40, 35, 30]);
        for (let lat = 35; lat <= 65; lat += 0.173) {
            for (let lon = 5; lon <= 6.5; lon += 0.0571) {
                const widest = reachBox(query({ start: [lon, lat], car: true, drive: 50, lift: 'helps' }))!;
                expect(boxCells(widest).length).toBeLessThanOrEqual(MAX_CELLS);
            }
        }
    });

    it('widens the start by the first leg, 5 km with lifts and 3 km for the last walk', () => {
        const half = (box: Box) => Math.round((box[3] - box[1]) / 2 * kmPerDegree * 10) / 10;
        expect(reachBox(query())).toBeUndefined();
        expect(half(reachBox(query({ start: START, walk: 1500 }))!)).toBe(4.5);
        expect(half(reachBox(query({ start: START, walk: 1500, lift: 'helps' }))!)).toBe(9.5);
        expect(half(reachBox(query({ start: START, car: true, drive: 20 }))!)).toBe(23);
        // 47° N caps the drive at 45 km.
        expect(half(reachBox(query({ start: START, car: true, drive: 50 }))!)).toBe(48);
    });

    it('loads the map view, or with a start point the part of it inside the widest reach', () => {
        const wide: Box = [5, 45, 11, 49], near: Box = [7.95, 46.95, 8.05, 47.05];
        expect(loadBox(query(), wide)).toBe(wide);
        const loaded = loadBox(query({ start: START }), wide)!;
        expect(boxCells(wide).length).toBeGreaterThan(MAX_CELLS);
        expect(boxCells(loaded).length).toBeLessThanOrEqual(MAX_CELLS);
        expect(half(loaded)).toBeCloseTo(53, 0);
        expect(loadBox(query({ start: START }), near)).toEqual(near);
        expect(loadBox(query({ start: START }), [10, 45, 11, 46])).toBeNull();

        function half(box: Box) { return (box[3] - box[1]) / 2 * kmPerDegree; }
    });
});

describe('rows', () => {
    // A lift from its valley 0.9 km east of the start to its top 3 km east, with a car park; car parks 2 km and 29 km north.
    const places: Record<string, FactsPlace> = {
        n1: { kind: 'parking', size: 'small', at_udeg: udeg(at(0, 2)) },
        n2: { kind: 'parking', size: 'large', at_udeg: udeg(at(0, 29)) },
        n3: { kind: 'parking', size: 'small', at_udeg: udeg(at(1)) },
        w4: { kind: 'lift', at_udeg: udeg(at(3)), valley_udeg: udeg(at(0.9)), parking: 'n3' },
        n5: { kind: 'peak', name: 'Gipfel', at_udeg: udeg(at(3)) },
    };
    // A loop near the start; a one-way route from the top of the lift down to its valley; a loop by the far car park.
    const loop = route(1, [at(0.5, -0.5), at(0.5, 0.5), at(1, 0), at(0.5, -0.5)]);
    const oneWay = route(2, [at(3), at(1.05)], { loop: false, length_m: 2500, ascent_m: 50, descent_m: 700, moving_s: 3000 });
    const far = route(3, [at(0, 30), at(1, 30)]);
    const loaded = files([loop, oneWay, far], {
        1: { parking: [['n1', 2100]] },
        2: { places: [['n5', 0]], lifts: [['w4', 0]], lift_ends: ['w4', 'end'], back_s: 5400 },
        3: { parking: [['n2', 900]] },
    }, places);

    it('makes a loop, a there and back and the two lift shapes, each with its numbers', () => {
        const found = find(query({ lift: 'helps' }), loaded);
        expect(found.map(({ record, shape, length_m, ascent_m, descent_m, moving_s }) => [record.id, shape, length_m, ascent_m, descent_m, moving_s])).toEqual([
            [1, 'loop', 10_000, 600, 600, 14_400],
            [2, 'back', 5000, 750, 750, 8400],
            // The valley is at the end: the hike up runs backwards.
            [2, 'liftDown', 2500, 700, 50, 5400],
            [2, 'liftUp', 2500, 50, 700, 3000],
            [3, 'loop', 10_000, 600, 600, 14_400],
        ]);
        expect(rows(find(query(), loaded))).toEqual(['1 loop', '2 back', '3 loop']);
        expect(rows(find(query({ lift: 'helps', shapes: ['liftUp'] }), loaded))).toEqual(['2 liftUp']);
        expect(rows(find(query({ length: [6000, 12_000] }), loaded))).toEqual(['1 loop', '3 loop']);
    });

    it('reaches a route on foot, by car or by lift, and a lift shape only at its valley', () => {
        const ways = (change: Partial<Query>) => Object.fromEntries(find(query({ start: START, ...change }), loaded)
            .map(row => [`${row.record.id} ${row.shape}`, row.ways.map(way => `${way.kind} ${way.cost.toFixed(1)}`)]));
        expect(ways({})).toEqual({ '1 loop': ['foot 2.0'] });
        // The far car park is 29 km away: a drive of 20 km does not reach it.
        expect(ways({ car: true, drive: 20, walk: 3000 })).toEqual({ '1 loop': ['foot 2.0', 'car 10.4'], '2 back': ['foot 4.2'] });
        expect(ways({ car: true, drive: 30 })).toEqual({ '1 loop': ['foot 2.0'], '3 loop': ['car 32.6'] });
        expect(ways({ lift: 'helps' })).toEqual({ '1 loop': ['foot 2.0'], '2 back': ['lift 4.6'], '2 liftDown': ['valley 4.6'], '2 liftUp': ['valley 4.6'] });
        // Always a lift: no walk to a route; the valley is beyond the walk, its car park 1 km away by car.
        expect(ways({ lift: 'always', car: true, walk: 500 })).toEqual({ '2 back': ['lift 2.0'], '2 liftDown': ['valley 2.0'], '2 liftUp': ['valley 2.0'] });
        // A pinned car park is the only start; the car park of a lift reaches its lift shapes, its top station none.
        expect(ways({ pin: 'n2' })).toEqual({ '3 loop': ['pin 3.6'] });
        expect(ways({ pin: 'n3', lift: 'helps' })).toEqual({ '2 liftDown': ['pin 1.0'], '2 liftUp': ['pin 1.0'] });
        expect(ways({ pin: 'w4', lift: 'helps' })).toEqual({ '2 back': ['pin 0.0'] });
    });

    it('keeps a route with no grade unless the rider asks for a strict grade', () => {
        const ungraded = files([route(1, [START, at(1)]), route(2, [START, at(1)], { hardest: undefined })], { 1: {}, 2: {} });
        expect(rows(find(query({ grade: [0, 2] }), ungraded))).toEqual(['1 loop', '2 loop']);
        expect(rows(find(query({ grade: [2, 5] }), ungraded))).toEqual(['2 loop']);
        expect(rows(find(query({ ungraded: false }), ungraded))).toEqual(['1 loop', '2 loop']);
        expect(rows(find(query({ grade: [0, 2], ungraded: false }), ungraded))).toEqual(['1 loop']);
        expect(counts(query({ grade: [0, 2], ungraded: false }), ungraded, []).ungraded).toBe(1);
    });
});

describe('order', () => {
    it('puts the most nice wishes first, then the least paved, the cheapest way, the route id and the shape', () => {
        const places: Record<string, FactsPlace> = { n9: { kind: 'hut', at_udeg: udeg(at(0)) } };
        const loaded = files([
            route(5, [START, at(0.4)]), route(4, [START, at(0.4)]), route(3, [at(0.2), at(1)]), route(2, [START, at(1)], { loop: false }), route(1, [START, at(1)]),
        ], {
            1: { places: [['n9', 0]], paved_pct: 40 }, 2: { places: [['n9', 0]], paved_pct: 5 }, 3: { paved_pct: 5 },
            4: { paved_pct: 5 }, 5: { paved_pct: 5, lift_ends: ['w8', 'start'] },
        }, { ...places, w8: { kind: 'lift', at_udeg: udeg(at(0.4)), valley_udeg: udeg(START) } });
        expect(rows(find(query({ wishes: { hut: 'nice' }, lift: 'helps' }), loaded))).toEqual(['2 back', '1 loop', '3 loop', '4 loop', '5 loop']);
        expect(rows(find(query({ wishes: { hut: 'nice' }, start: START, walk: 3000 }), loaded))).toEqual(['2 back', '1 loop', '4 loop', '5 loop', '3 loop']);
        const lift = files([route(6, [START, at(1)], { loop: false })], { 6: { lift_ends: ['w8', 'start'], back_s: 3000 } },
            { w8: { kind: 'lift', at_udeg: udeg(at(1)), valley_udeg: udeg(START) } });
        expect(rows(find(query({ lift: 'helps' }), lift))).toEqual(['6 back', '6 liftDown', '6 liftUp']);
        expect(rows(find(query({ lift: 'helps' }), lift, 'time'))).toEqual(['6 liftUp', '6 liftDown', '6 back']);
    });
});

describe('wishes', () => {
    it('maps a day to its day step from 1 September', () => {
        expect(['2026-09-01', '2026-09-02', '2026-09-03', '2027-01-15', '2027-08-31', '2028-08-31'].map(dayStep)).toEqual([0, 0, 1, 68, 182, 182]);
    });

    it('meets the snow wish inside the snow-free window, also across 1 September, and has no data without one', () => {
        const loaded = files([route(1, [START, at(1)]), route(2, [START, at(1)]), route(3, [START, at(1)]), route(4, [START, at(1)])],
            { 1: { snow_free: [170, 10] }, 2: { snow_free: [40, 120] }, 3: { snow_free: [] }, 4: {} });
        const must = (date: string) => find(query({ date, wishes: { snow: 'must' } }), loaded).map(row => row.record.id);
        expect(must('2026-08-25')).toEqual([1]);
        expect(must('2026-09-15')).toEqual([1]);
        expect(must('2026-10-15')).toEqual([]);
        expect(must('2027-02-01')).toEqual([2]);
        const nice = find(query({ date: '2026-09-15', wishes: { snow: 'nice' } }), loaded);
        expect(nice.map(row => [row.record.id, row.met, row.missed, row.unknown])).toEqual([
            [1, ['snow'], [], []], [2, [], ['snow'], []], [3, [], ['snow'], []], [4, [], [], ['snow']]]);
        const day = query({ date: '2026-09-15' }), told = counts(day, loaded, find(day, loaded)).wishes;
        expect([told.get('snow'), told.get('forest'), told.get('official'), told.get('quiet')]).toEqual([1, null, null, 4]);
    });
});

describe('suggestions', () => {
    // Loops `north` km from the start with a summit, free of snow, far from big roads, with a T2 part.
    const places: Record<string, FactsPlace> = { n1: { kind: 'parking', at_udeg: udeg(at(0, 10)) }, n2: { kind: 'peak', at_udeg: udeg(at(0, 0.5)) } };
    const loop = (id: number, north: number, fields: Partial<CatalogRecord> = {}) => route(id, [at(0, north), at(0.3, north)], fields);
    const fine: Partial<CatalogFacts> = { places: [['n2', 0]], snow_free: [0, 182] };
    const loaded = (records: CatalogRecord[], facts: Record<number, Partial<CatalogFacts>> = {}) =>
        files(records, Object.fromEntries(records.map(record => [record.id, { ...fine, ...facts[record.id] }])), places);
    const strict = query({ start: START, shapes: ['loop'], grade: [1, 2], wishes: { peak: 'must', quiet: 'must', snow: 'must' } });
    const tried = (found: ReturnType<typeof suggestions>) => found.map(one => [one.kind, one.count]);

    it('names the first four changes that give routes, in their order, and never the grade limit or the snow wish', () => {
        // Each route misses the query by one thing.
        const missing = loaded([loop(1, 0.5), loop(2, 0.5), loop(3, 0.5), loop(4, 0.5, { hardest: 0 }), loop(5, 0.5, { hardest: 4 }), loop(6, 1.5), loop(7, 10.2)],
            { 1: { places: [] }, 2: { big_road_pct: 50 }, 3: { snow_free: [] }, 7: { parking: [['n1', 200]] } });
        expect(find(strict, missing)).toEqual([]);
        const found = suggestions(strict, missing, 2000);
        expect(found.map(one => one.kind === 'nice' ? `nice ${one.wish}` : one.kind)).toEqual(['nice peak', 'nice quiet', 'part', 'walk']);
        expect(found.map(one => one.count)).toEqual([1, 1, 1, 1]);
        expect(found[3].query.walk).toBe(2000);
        // Only the snow wish and the grade limit keep these routes out.
        expect(suggestions(strict, loaded([loop(3, 0.5), loop(5, 0.5, { hardest: 4 })], { 3: { snow_free: [] } }), 2000)).toEqual([]);
    });

    it('drives 10 km farther up to the cap, adds a shape, removes the pin and widens the length', () => {
        const near = { start: START, car: true, drive: 5, shapes: ['loop' as const] };
        const routes = loaded([loop(7, 10.2), route(8, [at(0, 0.5), at(0.3, 0.5)], { loop: false })], { 7: { parking: [['n1', 200]] } });
        const drive = suggestions(query(near), routes, 2000);
        expect(tried(drive)).toEqual([['drive', 1], ['shape', 1]]);
        expect([drive[0].query.drive, drive[1].query.shapes]).toEqual([15, ['loop', 'back']]);
        expect(tried(suggestions(query({ ...near, drive: 10, pin: 'n9' }), routes, 2000))).toEqual([['unpin', 1]]);
        const length = suggestions(query({ length: [12_000, 14_000] }), loaded([loop(1, 0.5)]), 2000);
        expect(tried(length)).toEqual([['length', 1]]);
        expect(length[0].query.length).toEqual([10_000, 16_000]);
    });
});

describe('page address', () => {
    it('writes each value that is not the default, in a fixed order, and reads it again', () => {
        expect(queryParams(query(), TODAY)).toBe('');
        const set = query({
            date: '2026-07-14', start: [8.309613, 47.050171], walk: 1500, car: true, drive: 40, lift: 'helps', pin: 'n123', length: [10_000, 15_500],
            climb: [0, 800], shapes: ['loop', 'liftUp'], grade: [1, 3], ungraded: false, wishes: { 'via:w7': 'nice', hut: 'must', peak: 'nice', 'via:n9': 'nice' },
        });
        const text = 'date=2026-07-14&start=8.30961,47.05017&walk=1500&car=1&drive=40&lift=helps&pin=n123&length=10-15.5&climb=0-800'
            + '&shapes=loop,liftUp&grade=T2-T4&ungraded=0&nice=peak,via:n9,via:w7&must=hut&view=7.5,46.5,8.5,47.5';
        expect(queryParams(set, TODAY, [7.5, 46.5, 8.5, 47.5])).toBe(text);
        expect(readQuery(text, TODAY)).toEqual({ query: { ...set, start: [8.30961, 47.05017] }, view: [7.5, 46.5, 8.5, 47.5] });
        expect(readQuery('shapes=&length=5-', TODAY).query).toMatchObject({ shapes: [], length: [5000, Infinity] });
    });

    it('opens with the address, else the history, else the last query; a past day becomes today', () => {
        expect(openingQuery('walk=500', 'walk=700', 'walk=900', TODAY).query.walk).toBe(500);
        expect(openingQuery('', 'walk=700', 'walk=900', TODAY).query.walk).toBe(700);
        expect(openingQuery('', undefined, 'walk=900&date=2026-06-30', TODAY).query).toMatchObject({ walk: 900, date: TODAY });
        expect(openingQuery('', undefined, 'walk=99999&grade=T4-T2&pin=x', TODAY).query).toEqual(query());
    });
});
