import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import type { Coordinate } from './editor';
import type { BikeType } from './riding-profiles';
import { decodeCoordinates } from './route-answer';
import { nearestVertex, routePlan, searchRoutes, type Bounds, type CatalogRecord, type RouteQuery, type RouteRecord, type RouteShape, type RouteSort } from './signed-routes';

type VectorQuery = {
    start: Coordinate; radius_km: number; activity: BikeType; shape: RouteShape; distance_km?: Bounds; climb_m?: Bounds;
    hardest?: [number, number]; sort: RouteSort; covered?: string[];
};
const vector: {
    grid: string[];
    routes: CatalogRecord[];
    queries: { name: string; query: VectorQuery; expect: { id: number; distance_m: number }[]; cells: string[] }[];
    plans: { id: number; points_udeg: [number, number][]; turnarounds: number[]; place: Coordinate; nearest_vertex: number }[];
} = JSON.parse(readFileSync(new URL('../../../../../specs/vectors/signed-routes.json', import.meta.url), 'utf8'));
const routes = vector.routes;

const query = ({ start, radius_km, activity, shape, distance_km, climb_m, hardest, sort, covered }: VectorQuery): RouteQuery => ({
    start, radiusKm: radius_km, activity, shape, distanceKm: distance_km, climbM: climb_m, hardest, sort, covered: covered && new Set(covered),
});

const asked: string[] = [];
const search = (input: RouteQuery) => searchRoutes(input, async id => {
    asked.push(id);
    return vector.grid.includes(id) ? routes.filter(route => route.cells.includes(id)) : null;
});

describe('signed routes', () => {
    it.each(vector.queries)('$name', async ({ query: input, expect: found, cells }) => {
        asked.length = 0;
        const matches = await search(query(input));
        expect(asked.sort()).toEqual(cells);
        expect(matches.map(({ route, distanceM }) => ({ id: route.id, distance_m: Math.round(distanceM) }))).toEqual(found);
    });

    it('lists, at a smaller radius, the matches of a larger search within that radius', async () => {
        const wide = vector.queries.find(({ name }) => name.startsWith('Hiking within 50 km, least climb first'))!.query;
        const matches = await search(query(wide));
        for (const radius of [5, 10, 25]) {
            expect(await search(query({ ...wide, radius_km: radius }))).toEqual(matches.filter(match => match.distanceM <= radius * 1000));
        }
    });

    it.each(vector.plans)('plans route $id', ({ id, points_udeg, turnarounds, place, nearest_vertex }) => {
        const route = routes.find(route => route.id === id) as RouteRecord;
        expect(routePlan(route)).toEqual({ points: points_udeg.map(([lon, lat]) => [lon / 1e6, lat / 1e6]), turnarounds });
        expect(nearestVertex(decodeCoordinates(route.line_udeg), place)).toBe(nearest_vertex);
    });
});
