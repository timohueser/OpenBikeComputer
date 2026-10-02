import { afterEach, describe, expect, it, vi } from 'vitest';
import { createExpression } from '@maplibre/maplibre-gl-style-spec';
import type { LayerSpecification } from 'maplibre-gl';

afterEach(() => {
    vi.unstubAllGlobals();
    vi.resetModules();
});

// Every `kind` of the `landuse` layer in the planner tiles, sampled over twenty regions of Baden-Württemberg.
const tileKinds = [
    'aerodrome', 'airfield', 'allotments', 'bare_rock', 'beach', 'cemetery', 'college', 'commercial', 'dam', 'dog_park',
    'farmland', 'forest', 'garden', 'golf_course', 'grass', 'grassland', 'hospital', 'industrial', 'kindergarten', 'meadow',
    'military', 'national_park', 'nature_reserve', 'other', 'park', 'pedestrian', 'pier', 'pitch', 'platform', 'playground',
    'railway', 'recreation_ground', 'residential', 'runway', 'sand', 'school', 'scrub', 'taxiway', 'university',
    'village_green', 'wetland', 'wood', 'zoo',
];

async function landLayers(theme: 'light' | 'dark') {
    vi.stubGlobal('window', { location: { href: 'https://planner.example/plan/' } });
    const { mapStyle } = await import('./map-style');
    const layers = mapStyle(theme, 'dem://tiles', 'contours://tiles').layers;
    const kinds = (layer: LayerSpecification) => ('filter' in layer ? (layer.filter as [string, unknown, [string, string[]]])[2][1] : []);
    return { layers, land: layers.filter((layer) => layer.id.startsWith('land-')), kinds };
}

describe('planner land use style', () => {
    it.each(['light', 'dark'] as const)('draws every land use kind of the tiles once (%s)', async (theme) => {
        const { land, kinds } = await landLayers(theme);
        const drawn = land.flatMap(kinds);
        for (const kind of tileKinds) expect(drawn.filter((drawnKind) => drawnKind === kind), kind).toHaveLength(1);
    });

    it('shades the ground and the zones with the relief, and keeps water and small kinds flat above it', async () => {
        const { layers, land, kinds } = await landLayers('light');
        const ids = layers.map((layer) => layer.id);
        for (const terrain of ['relief', 'contour-lines']) {
            expect(ids.indexOf(terrain)).toBeGreaterThan(ids.indexOf('land-zone'));
            expect(ids.indexOf(terrain)).toBeLessThan(ids.indexOf('water'));
        }
        expect(ids.indexOf('land-ground')).toBeLessThan(ids.indexOf('land-zone'));
        expect(ids.indexOf('land-detail')).toBeGreaterThan(ids.indexOf('water'));
        expect(ids.indexOf('land-detail')).toBeLessThan(ids.indexOf('buildings'));
        const detail = land.find((layer) => layer.id === 'land-detail')!;
        expect(detail).toHaveProperty('minzoom', 13);
        expect(kinds(detail)).toEqual(expect.arrayContaining(['pitch', 'playground', 'kindergarten']));
    });

    it('keeps the region view calm: open ground is paper below zoom 9, forest fades in first', async () => {
        const { land } = await landLayers('light');
        const ground = land.find((layer) => layer.id === 'land-ground')!;
        const opacity = createExpression((ground as { paint: { 'fill-opacity': unknown } }).paint['fill-opacity'], 'paint_fill');
        expect(opacity.result).toBe('success');
        const at = (zoom: number, kind: string) => (opacity as { value: { evaluate: (globals: object, feature: object) => number } }).value.evaluate({ zoom }, { type: 'Polygon', properties: { kind } });
        expect(at(8, 'farmland')).toBe(0);
        expect(at(8, 'forest')).toBeGreaterThan(0.3);
        expect(at(12, 'farmland')).toBe(1);
    });
});
