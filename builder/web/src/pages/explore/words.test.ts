import { describe, expect, it } from 'vitest';
import type { CatalogFacts, CatalogRecord, FactsPlace } from '../../lib/planner/signed-routes';
import { defaultQuery, type Query, type Row, type Way } from './search';
import { accessSteps, accessText, hikeNumbers, snowText, suggestionText } from './words';

const PLACES: Record<string, FactsPlace> = {
    w1: { kind: 'parking', size: 'small', at_udeg: [0, 0] }, w2: { kind: 'parking', size: 'large', name: 'Talboden', at_udeg: [0, 0] },
    n3: { kind: 'lift', name: 'Brunni', at_udeg: [0, 0] }, n4: { kind: 'peak', name: 'Pilatus', at_udeg: [0, 0] },
};
const place = (id: string) => PLACES[id];
const record: CatalogRecord = { id: 1, kind: 'hiking', rank: 1, loop: true, length_m: 8000, ascent_m: 500, descent_m: 500, cells: [] };
const facts: CatalogFacts = { parking: [['w1', 40], ['w2', 250], ['w1', 900]], lifts: [['n3', 2000]], paved_pct: 5, big_road_pct: 0, water_pct: 0 };
const row = (way: Way, change: Partial<Row> = {}): Row => ({ record, facts, shape: 'loop', length_m: 8000, ascent_m: 500, descent_m: 500, ways: [way], met: [], missed: [], unknown: [], ...change });
const text = (steps: ReturnType<typeof accessSteps>) => steps.map(step => `${step.park ? 'P ' : step.icon ? `${step.icon} ` : ''}${step.text}`);
const query = (change: Partial<Query> = {}): Query => ({ ...defaultQuery('2026-07-01'), ...change });

describe('getting there', () => {
    it('names the large car park at the route with no start point, else the lift, else no car park', () => {
        expect(text(accessSteps(row({ kind: 'any', cost: 0 }), place))).toEqual(['P Large car park']);
        expect(accessText(row({ kind: 'any', cost: 0 }), place)).toBe('Talboden (large) is at the route.');
        expect(text(accessSteps(row({ kind: 'any', cost: 0 }, { facts: { ...facts, parking: [], lifts: [['n3', 100]] } }), place))).toEqual(['lift Lift']);
        expect(text(accessSteps(row({ kind: 'any', cost: 0 }, { facts: { ...facts, parking: [] } }), place))).toEqual(['No car park']);
        expect(accessSteps(row({ kind: 'any', cost: 0 }, { facts: undefined }), place)).toEqual([]);
    });

    it('lists the drive, the car park and a walk from 150 m, and the walks around a lift', () => {
        expect(text(accessSteps(row({ kind: 'car', park: 'w1', drive: 17_340, walk: 120, cost: 18 }), place))).toEqual(['car 17.3 km', 'P small']);
        expect(accessText(row({ kind: 'car', park: 'w2', drive: 17_340, walk: 860, cost: 21 }), place))
            .toBe('Drive 17.3 km to Talboden (large), then walk 900 m to the route.');
        expect(text(accessSteps(row({ kind: 'lift', lift: 'n3', before: 600, walk: 1450, cost: 9 }), place))).toEqual(['walk 600 m', 'lift Lift', 'walk 1.5 km']);
        expect(accessText(row({ kind: 'valley', lift: 'n3', before: 0, drive: 9000, cost: 10 }, { shape: 'liftDown' }), place))
            .toBe('Drive 9.0 km to the valley station of Brunni.');
    });
});

it('gives the numbers of a row in its shape: the descent of a lift shape, the time and a known grade', () => {
    const way: Way = { kind: 'any', cost: 0 };
    expect(hikeNumbers(row(way, { moving_s: 9000, record: { ...record, hardest: 1 } }))).toEqual(['8.0\u00a0km', '500\u00a0m up', '2h 30m', 'T2']);
    expect(hikeNumbers(row(way, { shape: 'liftDown', ascent_m: 900, descent_m: 100 }))).toEqual(['8.0\u00a0km', '900\u00a0m up', '100\u00a0m down']);
});

it('names each change that helps an empty list', () => {
    const before = query({ grade: [2, 4], walk: 1000 });
    expect(suggestionText({ kind: 'part', query: { ...before, grade: [0, 4] }, count: 3 }, before, place)).toBe('No T3 part needed');
    expect(suggestionText({ kind: 'walk', query: { ...before, walk: 2000 }, count: 3 }, before, place)).toBe('Walk up to 2 km');
    expect(suggestionText({ kind: 'nice', wish: 'snow', query: before, count: 1 }, before, place)).toBe('Free of snow as a wish only');
    expect(suggestionText({ kind: 'shape', shape: 'liftDown', query: before, count: 1 }, before, place)).toBe('Hike up, lift down too');
});

it('reads the snow window as days from 1 September, also across it', () => {
    expect(snowText(undefined)).toBeUndefined();
    expect(snowText([])).toBe('No day is usually free of snow');
    expect(snowText([0, 182])).toBe('Usually free of snow all year');
    expect(snowText([140, 30])).toBe('Usually free of snow 8 Jun to 1 Nov');
});
