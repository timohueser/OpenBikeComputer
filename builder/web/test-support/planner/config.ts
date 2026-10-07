import type { PlannerConfig } from '../../src/lib/planner/config';

/** The planner config of the unit suites, set in `vite.config.ts`: hosts that do not exist and no data layers. */
export const testConfig: PlannerConfig = {
    name: 'Test region',
    bounds: [7.45, 47.5, 10.5, 49.85],
    basemap: 'https://tiles.test/basemap.json',
    places: 'https://tiles.test/places.json',
    overlays: 'https://tiles.test/overlays.json',
    terrain: 'https://tiles.test/terrain/{z}/{x}/{y}.webp',
    attribution: 'Test map data',
    landcover_attribution: 'Test land cover',
    terrain_attribution: 'Test terrain',
    glyphs: 'https://tiles.test/fonts/{fontstack}/{range}.pbf',
    sprites: 'https://tiles.test/sprites',
    routing: 'https://api.test/routing',
    search: 'https://api.test/search',
    layers: {},
};
