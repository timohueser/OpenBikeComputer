import type { ExpressionSpecification, Map } from 'maplibre-gl';
import type { BikeType } from './riding-profiles';

/** Prototype: road layers from the file of `road-layers-prototype/build-data.py`, served next to the planner page. */
export type RoadLayer = 'steepness' | 'surface' | 'grade';
export const roadLayers: { id: RoadLayer; label: string }[] = [
    { id: 'steepness', label: 'Steepness' }, { id: 'surface', label: 'Surface' }, { id: 'grade', label: 'Trail grade' },
];
type Theme = 'light' | 'dark';
type Class = { label: string; light: string; dark: string; faint?: boolean };

// Climb-profile colours; the lightness falls from flat to steep in both themes.
const steepness: Class[] = [
    { label: '< 3 %', light: '#b2d6a0', dark: '#a2d192' },
    { label: '3–6 %', light: '#e0b21c', dark: '#d4a626' },
    { label: '6–9 %', light: '#e2781a', dark: '#de7426' },
    { label: '9–12 %', light: '#c8321f', dark: '#d8402e' },
    { label: '> 12 %', light: '#7a1018', dark: '#b02a32' },
];
// The router's surface classes, in order of `Surface` in planner/router/src/model.rs.
const surface: Class[] = [
    { label: 'Unknown', light: '#9a9a9a', dark: '#777777', faint: true },
    { label: 'Paved', light: '#0072b2', dark: '#4aa6e8' },
    { label: 'Compacted', light: '#56b4e9', dark: '#9ad3f5' },
    { label: 'Gravel', light: '#e69f00', dark: '#f0b43a' },
    { label: 'Dirt', light: '#d55e00', dark: '#f07a2a' },
    { label: 'Rough', light: '#cc79a7', dark: '#e59bc6' },
];
const ungraded: Class = { label: 'No grade', light: '#9a9a9a', dark: '#777777', faint: true };
// Swiss hiking signs: yellow, then white-red-white, then white-blue-white. `strolling` (0) draws as T1.
const sac: Class[] = [
    { label: 'T1', light: '#e0b000', dark: '#f2cf3a' },
    { label: 'T2', light: '#e2541c', dark: '#f27a4a' },
    { label: 'T3', light: '#a3121c', dark: '#e0404a' },
    { label: 'T4', light: '#4f8fe0', dark: '#7fb2f0' },
    { label: 'T5', light: '#1f4ea8', dark: '#4a7ee0' },
    { label: 'T6', light: '#14214f', dark: '#c8d6ff' },
];
// Ski-piste colours, as Trailforks draws them; S5 and S6 share the last class.
const mtb: Class[] = [
    { label: 'S0', light: '#3a9a3e', dark: '#5cc760' },
    { label: 'S1', light: '#2166ac', dark: '#5a9ae0' },
    { label: 'S2', light: '#c62828', dark: '#ef5350' },
    { label: 'S3', light: '#111111', dark: '#f2f2f2' },
    { label: 'S4', light: '#7b1fa2', dark: '#ce93d8' },
    { label: 'S5+', light: '#f08c00', dark: '#ffb340' },
];

/** The hiking grade for a walk, the MTB grade on every bike. */
const gradeKey = (bike: BikeType) => bike === 'hiking' ? 'sac' : 'mtb';

function classes(layer: RoadLayer, bike: BikeType): { key: string; rows: Class[]; index: ExpressionSpecification } {
    if (layer === 'steepness') return { key: 'slope', rows: steepness, index: ['get', 'slope'] };
    if (layer === 'surface') return { key: 'surface', rows: surface, index: ['get', 'surface'] };
    const key = gradeKey(bike);
    const rows = key === 'sac' ? sac : mtb;
    // Class 0 is "no grade"; a grade g is class 1 + g, clamped to the last row.
    const index: ExpressionSpecification = ['case', ['has', key],
        ['min', rows.length, ['+', 1, ['max', 0, ['-', ['get', key], key === 'sac' ? 1 : 0]]]], 0];
    return { key, rows: [ungraded, ...rows], index };
}

export function roadLegend(layer: RoadLayer, theme: Theme, bike: BikeType) {
    const { key, rows } = classes(layer, bike);
    const title = layer === 'grade' ? (key === 'sac' ? 'Hiking grade (SAC)' : 'MTB grade (mtb:scale)') : '';
    return { title, swatches: rows.map(row => ({ label: row.label, color: row[theme], faint: !!row.faint })) };
}

const source = 'road-layers-prototype', id = 'road-layer-prototype';

/** Shows one road layer, or none, under the labels and the route lines. */
export function syncRoadLayer(map: Map, layer: RoadLayer | '', theme: Theme, bike: BikeType) {
    if (map.getLayer(id)) map.removeLayer(id);
    if (!layer) return;
    if (!map.getSource(source)) map.addSource(source, { type: 'vector', url: `pmtiles://${new URL('road-layers-prototype.pmtiles', location.href).href}` });
    const { rows, index } = classes(layer, bike);
    const color = ['match', index, ...rows.flatMap((row, i) => [i, row[theme]]), '#ff00ff'] as unknown as ExpressionSpecification;
    const faintRows = rows.flatMap((row, i) => row.faint ? [i] : []);
    const faint = (faintRows.length ? ['match', index, faintRows, true, false] : false) as unknown as ExpressionSpecification;
    const width = (thin: number, full: number) => ['case', faint, thin, full] as unknown as ExpressionSpecification;
    map.addLayer({
        id, type: 'line', source, 'source-layer': 'roads', minzoom: 10,
        // Roads have no trail grade: the grade layer draws only paths and tracks.
        ...(layer === 'grade' ? { filter: ['==', ['get', 'path'], true] } : {}),
        layout: { 'line-cap': 'round', 'line-join': 'round', 'line-sort-key': index },
        paint: {
            'line-color': color,
            'line-width': ['interpolate', ['linear'], ['zoom'], 10, width(0.4, 1), 12, width(0.8, 2.2), 16, width(1.4, 4.5)],
        },
    }, map.getStyle().layers.find(layer => layer.type === 'symbol')?.id);
}

/** Ski-touring slope classes of `build-data.py --slope-out`; below 30° is clear. */
export const slopeAngleLegend = [
    { label: '30–35°', color: '#f2e500' }, { label: '35–40°', color: '#f59600' },
    { label: '40–45°', color: '#e0001a' }, { label: '> 45°', color: '#8228a0' },
];
const slopeId = 'slope-angle-prototype';

/** The slope angle of the terrain, over the relief and under the roads. */
export async function syncSlopeAngle(map: Map, shown: boolean) {
    if (map.getLayer(slopeId)) map.removeLayer(slopeId);
    if (!shown) return;
    if (!map.getSource(slopeId)) {
        const url = new URL('slope-angle-prototype.png', location.href).href;
        const { coordinates } = await (await fetch(`${url}.json`)).json();
        if (!map.getSource(slopeId)) map.addSource(slopeId, { type: 'image', url, coordinates });
    }
    if (map.getLayer(slopeId)) return;
    map.addLayer({ id: slopeId, type: 'raster', source: slopeId, paint: { 'raster-opacity': 0.55, 'raster-resampling': 'nearest', 'raster-fade-duration': 0 } },
        map.getStyle().layers.find(layer => layer.id.startsWith('roads_'))?.id);
}
