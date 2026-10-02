import { afterEach, describe, expect, it, vi } from 'vitest';
import { createPropertyExpression, latest } from '@maplibre/maplibre-gl-style-spec';
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

/** Evaluates a fill property expression of `layer` for a feature of `kind` at `zoom`, checked against the spec. */
function evaluate(layer: LayerSpecification, property: 'fill-opacity' | 'fill-sort-key', zoom: number, kind: string): number {
    const [group, spec] = property === 'fill-opacity' ? ['paint_fill', latest.paint_fill['fill-opacity']] : ['layout_fill', latest.layout_fill['fill-sort-key']];
    const expression = (layer as unknown as Record<string, Record<string, unknown>>)[group.split('_')[0]][property];
    const parsed = createPropertyExpression(expression, group, spec as Parameters<typeof createPropertyExpression>[2]);
    expect(parsed.result, property).toBe('success');
    return (parsed as { value: { evaluate: (globals: object, feature: object) => number } }).value.evaluate({ zoom }, { type: 'Polygon', properties: { kind } });
}

describe('planner land use style', () => {
    it.each(['light', 'dark'] as const)('draws every land use kind of the tiles once (%s)', async (theme) => {
        const { land, kinds } = await landLayers(theme);
        const drawn = land.flatMap(kinds);
        for (const kind of tileKinds) expect(drawn.filter((drawnKind) => drawnKind === kind), kind).toHaveLength(1);
    });

    it('shades the land with the relief, and keeps water and structures flat above it', async () => {
        const { layers, land, kinds } = await landLayers('light');
        const ids = layers.map((layer) => layer.id);
        const at = (id: string) => ids.indexOf(id);
        expect(at('land-ground')).toBeLessThan(at('land-zone'));
        expect(at('land-zone')).toBeLessThan(at('land-detail'));
        for (const terrain of ['relief', 'contour-lines']) {
            expect(at(terrain)).toBeGreaterThan(at('land-detail'));
            expect(at(terrain)).toBeLessThan(at('water'));
        }
        expect(at('land-structure')).toBeGreaterThan(at('water'));
        expect(at('land-structure')).toBeLessThan(at('buildings'));
        for (const id of ['land-detail', 'land-structure']) expect(land.find((layer) => layer.id === id)).toHaveProperty('minzoom', 13);
        expect(kinds(land.find((layer) => layer.id === 'land-detail')!)).toEqual(expect.arrayContaining(['pitch', 'playground', 'kindergarten']));
        expect(kinds(land.find((layer) => layer.id === 'land-structure')!)).toEqual(['platform', 'pier', 'dam']);
    });

    it('lifts forest over grass over fields over built-up land inside the ground, whatever the tile order', async () => {
        const { land } = await landLayers('light');
        const ground = land.find((layer) => layer.id === 'land-ground')!;
        const rank = (kind: string) => evaluate(ground, 'fill-sort-key', 14, kind);
        expect(rank('residential')).toBeLessThan(rank('farmland'));
        expect(rank('farmland')).toBeLessThan(rank('scrub'));
        expect(rank('scrub')).toBeLessThan(rank('forest'));
        expect(rank('meadow')).toBeLessThan(rank('wood'));
    });

    it('keeps the region view calm: open ground is paper below zoom 9, forest fades in first', async () => {
        const { land } = await landLayers('light');
        const ground = land.find((layer) => layer.id === 'land-ground')!;
        expect(evaluate(ground, 'fill-opacity', 8, 'farmland')).toBe(0);
        expect(evaluate(ground, 'fill-opacity', 8, 'forest')).toBeGreaterThan(0.3);
        expect(evaluate(ground, 'fill-opacity', 12, 'farmland')).toBe(1);
    });
});
