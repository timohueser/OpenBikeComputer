import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { recordCredits, routePlan, type CatalogRecord, type RouteRecord } from './signed-routes';

const vector: {
    routes: CatalogRecord[];
    plans: { id: number; points_udeg: [number, number][]; turnarounds: number[] }[];
} = JSON.parse(readFileSync(new URL('../../../../../specs/vectors/signed-routes.json', import.meta.url), 'utf8'));

describe('signed routes', () => {
    it.each(vector.plans)('plans route $id', ({ id, points_udeg, turnarounds }) => {
        const route = vector.routes.find(route => route.id === id) as RouteRecord;
        expect(routePlan(route)).toEqual({ points: points_udeg.map(([lon, lat]) => [lon / 1e6, lat / 1e6]), turnarounds });
    });

    it('gives the credit of a second source once', () => {
        const [first, second, osm] = vector.routes;
        const credit = 'Bundesamt für Strassen, Kanton, Stiftung SchweizMobil';
        expect(recordCredits([{ ...first, credit }, osm, { ...second, credit }])).toEqual([credit]);
        expect(recordCredits([{ ...first, credit: `${credit}; Other` }, { ...second, credit }])).toEqual([credit, 'Other']);
    });
});
