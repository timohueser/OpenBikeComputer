// The steepness and the trail grade of the ways that the router reads, drawn from the `ways` tiles of a release.
import type { DataDrivenPropertyValueSpecification, ExpressionSpecification, LineLayerSpecification, Map } from 'maplibre-gl';
import { gradeBands, gradeThresholds } from './grade-data';
import type { Legend, Swatch, Theme } from './layers/data-layer';
import { releaseUrl } from './release';

export type WayLayer = 'steepness' | 'trail-grade';

/** The tile values of the property that draw in one colour. */
interface WayClass { label: string; light: string; dark: string; values: number[] }
/** A layer on one property of the `ways` tiles; a way with no value of it is a thin line in the colour of `none`. */
interface WayScale { label: string; property: 'slope' | 'mtb' | 'sac'; classes: WayClass[]; none: Omit<WayClass, 'values'>; trails: boolean }

const band = ({ label, color, dark }: (typeof gradeBands)[number]) => ({ label, light: color, dark });
const ungraded = { label: 'No grade', light: '#9a9a9a', dark: '#777777' };

/** The flat band and the climb bands of the elevation profile: a way on the map has no direction. */
const steepness: WayScale = {
    label: 'Steepness', property: 'slope', trails: false, none: band(gradeBands.at(-1)!),
    classes: gradeBands.slice(gradeThresholds.length, -1).map((row, value) => ({ ...band(row), values: [value] })),
};
const blue = { light: '#2166ac', dark: '#5a9ae0' }, red = { light: '#c62828', dark: '#ef5350' };
/** `mtb:scale` in the colours of ski pistes: green, blue, red, and black from S3. */
const mtb: WayScale = {
    label: 'MTB grade', property: 'mtb', trails: true, none: ungraded,
    classes: [
        { label: 'S0', light: '#3a9a3e', dark: '#5cc760', values: [0] },
        { label: 'S1', ...blue, values: [1] },
        { label: 'S2', ...red, values: [2] },
        { label: 'S3+', light: '#111111', dark: '#f2f2f2', values: [3, 4, 5, 6] },
    ],
};
/** `sac_scale` in the colours of the Swiss hiking signs: yellow, white-red-white, white-blue-white. `strolling`, 0, draws as T1. */
const sac: WayScale = {
    label: 'Hiking grade', property: 'sac', trails: true, none: ungraded,
    classes: [
        { label: 'T1', light: '#e0b000', dark: '#f2cf3a', values: [0, 1] },
        { label: 'T2–T3', ...red, values: [2, 3] },
        { label: 'T4–T6', ...blue, values: [4, 5, 6] },
    ],
};

/** The trail grade is the hiking grade with the Hiking profile and the MTB grade with each other profile. */
export const wayScale = (layer: WayLayer, hiking: boolean): WayScale => layer === 'steepness' ? steepness : hiking ? sac : mtb;

/** The name of a layer and its legend. The steepness names the ways with no class last, as the profile; a trail with no grade stands first. */
export function wayLegend(layer: WayLayer, hiking: boolean, theme: Theme): { label: string; legend: Legend } {
    const { label, classes, none, trails } = wayScale(layer, hiking);
    const lines: Swatch[] = classes.map(row => ({ label: row.label, color: row[theme], shape: 'line' }));
    const thin: Swatch = { label: none.label, color: none[theme], shape: 'thin' };
    return { label, legend: { swatches: trails ? [thin, ...lines] : [...lines, thin] } };
}

const SOURCE = 'ways', LINE = 'ways', CASING = 'ways-casing';
/** The zoom of the first `ways` tiles: the map draws no way below it. */
export const WAYS_ZOOM = 12;

/**
 * Shows one layer of the ways, or none, over the roads of the basemap and under its labels. A layer that is off has
 * no style layer, so MapLibre requests no tile of the source. `url` is a TileJSON, or a PMTiles archive of a local release.
 */
export function syncWays(map: Map, url: string, shown: { layer: WayLayer; hiking: boolean; theme: Theme } | null) {
    for (const id of [LINE, CASING]) if (map.getLayer(id)) map.removeLayer(id);
    if (!shown) return;
    if (!map.getSource(SOURCE)) map.addSource(SOURCE, { type: 'vector', url: url.endsWith('.json') ? releaseUrl(url) : `pmtiles://${url}` });
    const { property, classes, none, trails } = wayScale(shown.layer, shown.hiking), theme = shown.theme;
    const value: ExpressionSpecification = ['coalesce', ['get', property], -1];
    const width = (extra: number): DataDrivenPropertyValueSpecification<number> =>
        ['interpolate', ['linear'], ['zoom'], WAYS_ZOOM, ['case', ['has', property], 2.2 + extra, 0.8 + extra], 16, ['case', ['has', property], 4.5 + extra, 1.4 + extra]];
    const layer = (id: string, paint: LineLayerSpecification['paint']): LineLayerSpecification => ({
        id, type: 'line', source: SOURCE, 'source-layer': 'ways', minzoom: WAYS_ZOOM,
        ...(trails ? { filter: ['==', ['get', 'trail'], true] } : {}),
        // A harder class lies over an easier one where two ways meet.
        layout: { 'line-cap': 'round', 'line-join': 'round', 'line-sort-key': value },
        paint,
    });
    const before = map.getStyle().layers.find(layer => layer.type === 'symbol')?.id;
    // A grade line has 1 px of the map background on each side, so it stays readable on a band of the signed routes.
    if (trails) map.addLayer(layer(CASING, { 'line-color': map.getPaintProperty('background', 'background-color') as string, 'line-width': width(2) }), before);
    map.addLayer(layer(LINE, {
        'line-color': ['match', value, ...classes.flatMap(row => [row.values, row[theme]]), none[theme]] as ExpressionSpecification,
        'line-width': width(0),
    }), before);
}
