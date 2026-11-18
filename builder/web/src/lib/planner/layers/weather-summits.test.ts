import { describe, expect, it, vi } from 'vitest';
import type { TilePlace } from '../place-groups';

const tile: TilePlace[] = [
    { id: 1, kind: 'summit', category: 'peak', name: 'Pilatus', lon: 8.25, lat: 46.98, ele: 2128 },
    { id: 2, kind: 'fountain', category: 'water', name: 'Brunnen', lon: 8.26, lat: 46.98 },
];
vi.mock('../map-data', () => ({ config: { places: 'https://tiles.example/places.json', terrain: 'https://tiles.example/terrain/{z}/{x}/{y}.webp' } }));
vi.mock('../place-index', () => ({ placeTileKeys: vi.fn(async () => ['11/1071/720']), placeTile: vi.fn(async () => tile) }));

describe('weather summits', () => {
    it('labels the summits of the place tiles of the view from zoom 12, whatever kinds the map shows', async () => {
        const { viewSummits } = await import('./weather-layer.svelte');
        const { mapPlaces } = await import('./weather');
        expect(mapPlaces(await viewSummits([8.2, 46.9, 8.3, 47], 12))).toEqual([
            { name: 'Pilatus', coordinate: [8.25, 46.98], peak: true, population: 0, rank: 0, minZoom: 12, elevation: 2128 },
        ]);
        expect(await viewSummits([8.2, 46.9, 8.3, 47], 11.9)).toEqual([]);
    });
});
