import { describe, expect, it } from 'vitest';
import { kmPerDegree, type Coordinate } from '../../lib/planner/geo';
import type { CatalogFacts, CatalogRecord, FactsPlace } from '../../lib/planner/signed-routes';
import { defaultQuery, find, type Loaded, type Query } from './search';
import { MARKER_LAYERS, mapData, markerClick, markers } from './search-map';

const START: Coordinate = [8, 47];
const at = (east: number, north = 0): Coordinate => [START[0] + east / (kmPerDegree * Math.cos(47 * Math.PI / 180)), START[1] + north / kmPerDegree];
const udeg = ([lon, lat]: Coordinate): [number, number] => [Math.round(lon * 1e6), Math.round(lat * 1e6)];

function route(id: number, points: Coordinate[], fields: Partial<CatalogRecord> = {}): CatalogRecord {
    const line = points.map(udeg).flatMap(([lon, lat], i, all) => i ? [lon - all[i - 1][0], lat - all[i - 1][1]] : [lon, lat]);
    return { id, kind: 'hiking', rank: 1, loop: true, length_m: 10_000, ascent_m: 600, descent_m: 600, moving_s: 14_400, hardest: 1, cells: [], line_udeg: line, ...fields };
}

// A loop 5 km east of the start with a small car park at the route, a large one 200 m away and one 2 km away; a lift whose
// top is on a second loop; a summit and a hut on the first loop, a lake on the second.
const places: Record<string, FactsPlace> = {
    n1: { kind: 'parking', size: 'small', at_udeg: udeg(at(5)) },
    n2: { kind: 'parking', size: 'large', at_udeg: udeg(at(5, 0.2)) },
    n3: { kind: 'parking', size: 'small', at_udeg: udeg(at(5, 2)) },
    w4: { kind: 'lift', at_udeg: udeg(at(10)), valley_udeg: udeg(at(8)) },
    n5: { kind: 'peak', at_udeg: udeg(at(6)) },
    n6: { kind: 'hut', at_udeg: udeg(at(6, 1)) },
    w7: { kind: 'lake', at_udeg: udeg(at(10, 1)) },
};
const facts: Record<number, Partial<CatalogFacts>> = {
    1: { parking: [['n1', 0], ['n2', 200], ['n3', 2000]], places: [['n5', 0], ['n6', 20]] },
    2: { lifts: [['w4', 0]], places: [['w7', 40]] },
};
const loaded: Loaded = {
    records: [route(1, [at(5), at(6), at(6, 1), at(5)]), route(2, [at(10), at(11), at(10, 1), at(10)])],
    factsOf: id => facts[id] && { paved_pct: 0, big_road_pct: 0, water_pct: 0, ...facts[id] },
    place: id => places[id],
};
const query = (change: Partial<Query> = {}): Query => ({ ...defaultQuery('2026-07-01'), ...change });
const marked = (change: Partial<Query>, hot?: number) => {
    const rows = find(query(change), loaded);
    return Object.fromEntries(markers(rows, query(change), rows.find(row => row.record.id === hot), id => places[id]));
};

describe('markers', () => {
    it('marks the car park or lift that each row uses: with no start point the one that the row names', () => {
        // The large car park within 300 m of the route, and the lift whose top is on the route.
        expect(marked({})).toEqual({ n2: 'quiet', w4: 'quiet' });
        // With a start point and the car, the car park of the cheapest way only: n1 at the route before n2 and n3, whose
        // walk is too long. A walk uses none.
        expect(marked({ start: at(5, 2.1), car: true })).toEqual({ n1: 'quiet' });
        expect(marked({ start: at(5.5) })).toEqual({});
    });

    it('marks a place of a wished kind that a row passes, each place of the hot row, and a pinned or passed place above all', () => {
        expect(marked({ wishes: { peak: 'nice' } })).toEqual({ n2: 'quiet', w4: 'quiet', n5: 'quiet' });
        expect(marked({ wishes: { peak: 'nice' } }, 1)).toEqual({ n2: 'hot', w4: 'quiet', n5: 'hot', n6: 'hot' });
        expect(marked({ wishes: { 'via:w7': 'must' } })).toEqual({ w4: 'quiet', w7: 'pin' });
        expect(marked({ pin: 'n2' })).toEqual({ n2: 'pin' });
    });
});

describe('the layers of the markers', () => {
    it('keeps the start point out of the click, lets places hide each other, and keeps the used car parks', () => {
        const roles = (change: Partial<Query>) => mapData(find(query(change), loaded), query(change), undefined, id => places[id]).features
            .map(({ properties }) => `${properties!.role} ${properties!.id ?? ''}`.trim());
        expect(roles({ start: at(5, 2.1), car: true, wishes: { peak: 'nice', 'via:w7': 'nice' } }))
            .toEqual(['used n1', 'places n5', 'pin w7', 'start', 'reach']);
        expect(roles({ wishes: {} })).toEqual(['trailhead n2', 'trailhead w4', 'cable', 'valley']);
        // A layer that takes a click holds only features with an id.
        expect(MARKER_LAYERS.map(layer => layer.slice(8))).not.toContain('start');
    });
});

describe('a click on a marker', () => {
    const place = (id: string) => places[id];

    it('pins a car park or a top station as the only start, and unpins it', () => {
        expect(markerClick('n3', query(), place)).toEqual({ pin: 'n3' });
        expect(markerClick('w4', query({ pin: 'n3' }), place)).toEqual({ pin: 'w4' });
        expect(markerClick('n3', query({ pin: 'n3' }), place)).toEqual({ pin: undefined });
    });

    it('asks for the routes that pass a place, and gives the others back', () => {
        expect(markerClick('n5', query({ wishes: { hut: 'nice' } }), place)).toEqual({ wishes: { hut: 'nice', 'via:n5': 'must' } });
        expect(markerClick('n5', query({ wishes: { 'via:n5': 'must' } }), place)).toEqual({ wishes: {} });
        // A "Passes" wish that is nice becomes a must.
        expect(markerClick('n5', query({ wishes: { 'via:n5': 'nice' } }), place)).toEqual({ wishes: { 'via:n5': 'must' } });
    });
});
