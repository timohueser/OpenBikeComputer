import { describe, expect, it } from 'vitest';
import { routesPlaces } from './presentation';
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
