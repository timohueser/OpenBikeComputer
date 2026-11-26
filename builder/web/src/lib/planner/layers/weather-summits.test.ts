import { describe, expect, it, vi } from 'vitest';
import type { TilePlace } from '../place-groups';

const tile: TilePlace[] = [
    { id: 1, kind: 'summit', category: 'peak', name: 'Pilatus', lon: 8.25, lat: 46.98, ele: 2128 },
    { id: 2, kind: 'fountain', category: 'water', name: 'Brunnen', lon: 8.26, lat: 46.98 },
];
vi.mock('../map-data', () => ({ config: { places: 'https://tiles.example/places.json', terrain: 'https://tiles.example/terrain/{z}/{x}/{y}.webp' } }));
vi.mock('../place-index', () => ({ placeTileKeys: vi.fn(async () => ['11/1071/720']), placeTile: vi.fn(async () => tile) }));
// Each place has typical rain, so each peak takes a label.
vi.mock('./weather', async original => ({ ...await original<object>(), rainAt: () => 12 }));

describe('weather summits', () => {
    it('labels the summits of the place tiles of the view, which the map style has no source for', async () => {
        const { weatherLayer } = await import('./weather-layer.svelte');
        const setData = vi.fn();
        const map = {
            getSource: () => ({ setData }), getZoom: () => 12, getBounds: () => ({ toArray: () => [[8.2, 46.9], [8.3, 47]] }),
            // The basemap has no town here, and the style has no `places` source.
            querySourceFeatures: () => [],
        };
        const layer = Object.assign(weatherLayer('https://tiles.example/climate.json'), {
            map, shown: true, date: '2026-07-01', climate: { version: 1, meta: { firstYear: 2015 }, loaded: () => undefined },
        });
        layer.variable!.value = 'rain';
        await (layer as unknown as { relabel: () => Promise<void> }).relabel();
        expect(setData.mock.lastCall?.[0].features.map((feature: { properties: { name: string; peak: boolean } }) => feature.properties))
            .toEqual([expect.objectContaining({ name: 'Pilatus', peak: true })]);
    });
});
