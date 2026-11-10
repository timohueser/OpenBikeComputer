import { describe, expect, it } from 'vitest';
import { contract, routesPlaces, stayKind } from './presentation';
import type { SearchPlace } from './types';

const place = (name: string, kind: string): SearchPlace => ({ source: `${name}-${kind}`, name, kind, lon: 8, lat: 49, city: '', region: '', distance: 0, precision: 'place' });

describe('signed routes place', () => {
    it('starts from the largest settlement of the typed name, not the nearest result', () => {
        const results = [place('Weinheimer Straße', 'bus_stop'), place('Weinheim', 'train_station'), place('Weinheim', 'city'), place('Weinheim', 'district')];
        expect(routesPlaces(results, 'Weinheim').map(p => p.kind)).toEqual(['city', 'district', 'train_station', 'bus_stop']);
        expect(routesPlaces([place('Titisee-Neustadt', 'town'), place('Titisee', 'lake')], 'Titisee')[0].kind).toBe('lake');
        expect(routesPlaces([], 'Titisee')).toEqual([]);
    });
});

describe('kinds of places to sleep', () => {
    it('gives each place kind that search finds to sleep in one of the five kinds', () => {
        const sleepKinds = Object.values(contract.kinds).filter(kind => kind.parent === 'sleep').flatMap(kind => kind.data);
        expect(sleepKinds.length).toBeGreaterThan(0);
        for (const kind of sleepKinds) expect(stayKind(kind), kind).toBeDefined();
    });
});
