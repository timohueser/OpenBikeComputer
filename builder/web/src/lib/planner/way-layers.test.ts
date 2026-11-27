import { expect, it } from 'vitest';
import { createPropertyExpression, v8, type StylePropertySpecification } from '@maplibre/maplibre-gl-style-spec';
import type { LineLayerSpecification, Map } from 'maplibre-gl';
import { gradeBands } from './grade-data';
import { syncWays, wayLegend } from './way-layers';

/** A map with a road layer and a label layer of the basemap; `layers` are the style layers in their order. */
function fakeMap() {
    const layers: LineLayerSpecification[] = [{ id: 'roads' }, { id: 'labels', type: 'symbol' }] as unknown as LineLayerSpecification[];
    const sources: Record<string, unknown> = {};
    const map = {
        getLayer: (id: string) => layers.find(layer => layer.id === id), removeLayer: (id: string) => { layers.splice(layers.findIndex(layer => layer.id === id), 1); },
        getSource: (id: string) => sources[id], addSource: (id: string, source: unknown) => { sources[id] = source; },
        addLayer: (layer: LineLayerSpecification, before: string) => { layers.splice(layers.findIndex(item => item.id === before), 0, layer); },
        getStyle: () => ({ layers }), getPaintProperty: () => '#f4f2eb',
    } as unknown as Map;
    return { map, layers, sources };
}

/** The colour that a line layer draws a way of the tiles in. */
function colour(layer: LineLayerSpecification, properties: Record<string, unknown>): string {
    const parsed = createPropertyExpression(layer.paint!['line-color'], 'line-color', v8.paint_line['line-color'] as StylePropertySpecification);
    if (parsed.result === 'error') throw new Error(parsed.value[0].message);
    return String(parsed.value.evaluate({ zoom: 13 }, { type: 2, properties }));
}

it('gives the steepness the flat band and the climb bands of the elevation profile, and names the ways with no class', () => {
    for (const theme of ['light', 'dark'] as const) {
        const bands = [...gradeBands.slice(5, 11).map(row => ({ ...row, shape: 'line' })), { ...gradeBands[11], shape: 'thin' }];
        expect(wayLegend('steepness', false, theme)).toEqual({ label: 'Steepness', legend: { swatches: bands.map(row => ({ label: row.label, color: theme === 'dark' ? row.dark : row.color, shape: row.shape })) } });
    }
    const { map, layers } = fakeMap();
    syncWays(map, 'https://tiles.test/ways.json', { layer: 'steepness', hiking: false, theme: 'light' });
    expect(layers.map(layer => layer.id)).toEqual(['roads', 'ways', 'labels']);
    expect(layers[1].filter).toBeUndefined();
    const drawn = [0, 1, 2, 3, 4, 5].map(slope => colour(layers[1], { slope }));
    expect(new Set(drawn).size).toBe(6);
    expect(drawn).not.toContain(colour(layers[1], {}));
});

it('gives the Hiking profile the hiking grades and each other profile the MTB grades', () => {
    const labels = (hiking: boolean) => { const { label, legend } = wayLegend('trail-grade', hiking, 'light'); return [label, ...('swatches' in legend ? legend.swatches.map(swatch => swatch.label) : [])]; };
    expect(labels(true)).toEqual(['Hiking grade', 'No grade', 'T1', 'T2–T3', 'T4–T6']);
    expect(labels(false)).toEqual(['MTB grade', 'No grade', 'S0', 'S1', 'S2', 'S3+']);
});

it('draws the trails only, each on a casing, with the hiking grades in the three sign colours and the MTB grades from S3 as S3+', () => {
    const { map, layers, sources } = fakeMap();
    syncWays(map, 'https://tiles.test/ways.json', { layer: 'trail-grade', hiking: true, theme: 'light' });
    expect(layers.map(layer => layer.id)).toEqual(['roads', 'ways-casing', 'ways', 'labels']);
    expect(layers.slice(1, 3).map(layer => layer.filter)).toEqual(Array(2).fill(['==', ['get', 'trail'], true]));
    const sac = (value?: number) => colour(layers[2], value === undefined ? { mtb: 2 } : { sac: value });
    expect([0, 1, 2, 3, 4, 5, 6].map(sac)).toEqual([0, 0, 1, 1, 2, 2, 2].map(index => [sac(1), sac(2), sac(4)][index]));
    expect(new Set([undefined, 1, 2, 4].map(sac)).size).toBe(4);

    syncWays(map, 'https://tiles.test/ways.json', { layer: 'trail-grade', hiking: false, theme: 'light' });
    const mtb = (value?: number) => colour(layers[2], value === undefined ? { sac: 2 } : { mtb: value });
    expect([4, 5, 6].map(mtb)).toEqual(Array(3).fill(mtb(3)));
    expect(new Set([undefined, 0, 1, 2, 3].map(mtb)).size).toBe(5);

    // A layer that is off has no style layer; the source stays for the next one.
    syncWays(map, 'https://tiles.test/ways.json', null);
    expect(layers.map(layer => layer.id)).toEqual(['roads', 'labels']);
    expect(Object.keys(sources)).toEqual(['ways']);
});
